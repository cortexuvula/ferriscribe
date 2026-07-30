//! Per-user spellcheck dictionary handlers for the `/v1/user-dictionary`
//! routes.
//!
//! Reads/writes hit `medical_db::user_dictionary::UserDictionaryRepo`
//! against the office server's local SQLite DB. Same bearer auth +
//! spawn_blocking pattern as the vocab handlers. No PHI in logs — only
//! word lengths, counts, and boolean outcomes are ever logged; the word
//! value itself never reaches a log line.
//!
//! In addition to the original list/add/remove CRUD, this module serves the
//! sync surface that mirrors `/v1/condition-chips`: a `POST /sync` two-way
//! merge and a `GET /events` SSE stream. See [`dict_sync_handler`] and
//! [`dict_events_handler`].

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use futures_util::Stream;
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::{ApiState, authorize};

#[derive(Deserialize)]
pub(super) struct DictAddBody {
    word: String,
}

pub(super) async fn dict_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let words = tokio::task::spawn_blocking(
        move || -> Result<Vec<String>, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::list(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api list failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!(count = words.len(), "dict_api: list");
    Ok(Json(words))
}

pub(super) async fn dict_add_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<DictAddBody>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word = body.word;
    let word_len = word.len();
    let now_iso = now_iso();
    let added =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word, &now_iso)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api add failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, added, "dict_api: add");
    // Best-effort notify SSE subscribers (no receivers is not an error).
    let _ = state.dict_changed_tx.send(());
    Ok(Json(added))
}

pub(super) async fn dict_remove_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(word): Path<String>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word_len = word.len();
    let now_iso = now_iso();
    let removed =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word, &now_iso)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api remove failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, removed, "dict_api: remove");
    // Best-effort notify SSE subscribers (no receivers is not an error).
    let _ = state.dict_changed_tx.send(());
    Ok(Json(removed))
}

/// POST /v1/user-dictionary/sync — two-way merge.
///
/// Body: the client's full entry list (active words + tombstones).
/// Returns: the merged active word list after applying last-write-wins.
///
/// Mirrors `/v1/condition-chips/sync`. Tombstones older than 30 days are
/// pruned opportunistically (best-effort — a prune failure must not fail the
/// sync). Fires `dict_changed_tx` so SSE subscribers refresh.
pub(super) async fn dict_sync_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let incoming_count = incoming.len();

    // Prune old tombstones opportunistically (30 days). Best-effort — a
    // prune failure must not fail the sync.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let cutoff_iso = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let merged = tokio::task::spawn_blocking(
        move || -> Result<Vec<String>, medical_core::error::AppError> {
            let conn = db.conn()?;
            let result =
                medical_db::user_dictionary::UserDictionaryRepo::merge_incoming(&conn, &incoming)
                    .map_err(medical_core::error::AppError::from)?;
            // Best-effort prune — don't fail the sync if pruning errors.
            let _ = medical_db::user_dictionary::UserDictionaryRepo::prune_tombstones(
                &conn,
                &cutoff_iso,
            );
            Ok(result)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api sync failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Notify SSE subscribers that the dictionary changed. Best-effort: no
    // receivers is not an error (send returns Err only when there are no
    // active receivers, which is the normal idle case).
    let _ = state.dict_changed_tx.send(());

    info!(
        incoming_count,
        result_count = merged.len(),
        "dict_api: sync"
    );
    Ok(Json(merged))
}

/// GET /v1/user-dictionary/events — Server-Sent Events stream.
///
/// Pushes a `data: connected` event immediately on connection, then a
/// `data: changed` event each time a user-dictionary sync/add/remove
/// completes on the server. Clients use this to refresh their local word
/// list in near-realtime. The stream stays open until the client disconnects
/// or the server shuts down.
pub(super) async fn dict_events_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let mut rx = state.dict_changed_tx.subscribe();
    let stream = async_stream::stream! {
        yield Ok(Event::default().data("connected"));
        loop {
            match rx.recv().await {
                Ok(()) => yield Ok(Event::default().data("changed")),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream))
}

/// ISO 8601 UTC timestamp with millisecond precision, matching the format
/// used by dictionary rows.
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
