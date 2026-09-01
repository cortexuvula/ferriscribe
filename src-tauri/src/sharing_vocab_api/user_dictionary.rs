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
//! merge (legacy wire shape — `Vec<String>` of active words), a
//! `POST /sync-full` full-fidelity merge whose response carries tombstones
//! so deletions propagate to every client, and a `GET /events` SSE stream.
//! See [`dict_sync_handler`], [`dict_sync_full_handler`], and
//! [`dict_events_handler`].

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::{ApiState, authorize};

#[derive(Deserialize)]
pub(super) struct DictAddBody {
    word: String,
}

pub(super) async fn dict_list_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
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

pub(super) async fn dict_add_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
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

pub(super) async fn dict_remove_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
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

/// POST /v1/user-dictionary/sync — two-way merge (legacy wire shape).
///
/// Body: the client's full entry list (active words + tombstones).
/// Returns: the merged active word list after applying last-write-wins.
///
/// Mirrors `/v1/condition-chips/sync`. Tombstones older than 365 days are
/// pruned opportunistically (best-effort — a prune failure must not fail the
/// sync). Fires `dict_changed_tx` so SSE subscribers refresh.
///
/// The `Vec<String>` response cannot carry tombstones back to clients, so
/// deletions only propagate server-side here; full-fidelity propagation
/// (both directions) lives in [`dict_sync_full_handler`]. The legacy shape
/// is kept so old clients keep working unchanged.
pub(super) async fn dict_sync_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let incoming_count = incoming.len();

    // Prune old tombstones opportunistically (365 days — matches
    // `/sync-full` and the condition-chips sync: a tombstone must outlive
    // every stale client's next sync, or a machine that syncs rarely would
    // never learn of the deletion and could resurrect the word). Best-effort
    // — a prune failure must not fail the sync.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(365);
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

/// POST /v1/user-dictionary/sync-full — full-fidelity two-way merge.
///
/// Body: the client's full entry list (active words + tombstones).
/// Returns: the merged FULL entry list (active words + tombstones) after
/// applying last-write-wins. Tombstones travel intentionally so a deletion
/// on the server reaches every client; each client merges the response
/// locally via `merge_incoming`, which applies the tombstones and returns
/// the active list for its UI.
///
/// Mirrors `/v1/condition-chips/sync`. Clients running against an older
/// server get a 404/405 here and fall back to the legacy `/sync` (see
/// `UserDictRemote::sync_full`). Tombstones older than 365 days are pruned
/// opportunistically (best-effort — a prune failure must not fail the
/// sync). Fires `dict_changed_tx` so SSE subscribers refresh.
pub(super) async fn dict_sync_full_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>,
) -> Result<Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);

    let incoming_count = incoming.len();

    // Prune old tombstones opportunistically. Retention is 365 days: a
    // tombstone must outlive every stale client's next sync, or a machine
    // that syncs rarely (or was offline for weeks) would never learn of the
    // deletion and could resurrect the word practice-wide. The dictionary
    // table is tiny, so a year of tombstones costs nothing. Best-effort —
    // a prune failure must not fail the sync.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(365);
    let cutoff_iso = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let merged = tokio::task::spawn_blocking(
        move || -> Result<Vec<medical_core::types::user_dict_entry::UserDictEntry>, medical_core::error::AppError> {
            let conn = db.conn()?;
            // Merge the client's list in (LWW; ties break toward the
            // tombstone). The merge's return value is the ACTIVE list —
            // discard it and serve the FULL list so tombstones travel.
            medical_db::user_dictionary::UserDictionaryRepo::merge_incoming(&conn, &incoming)
                .map_err(medical_core::error::AppError::from)?;
            // Best-effort prune — don't fail the sync if pruning errors.
            let _ = medical_db::user_dictionary::UserDictionaryRepo::prune_tombstones(
                &conn,
                &cutoff_iso,
            );
            medical_db::user_dictionary::UserDictionaryRepo::list_all(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api sync-full failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Notify SSE subscribers that the dictionary changed. Best-effort: no
    // receivers is not an error (send returns Err only when there are no
    // active receivers, which is the normal idle case).
    let _ = state.dict_changed_tx.send(());

    info!(
        incoming_count,
        result_count = merged.len(),
        "dict_api: sync-full"
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
pub(super) async fn dict_events_handler<R: tauri::Runtime>(
    AxumState(state): AxumState<ApiState<R>>,
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
    // Keep-alive comments (`:\n\n` every 15s by default) prevent NAT / relay
    // idle timeouts from silently dropping the long-lived SSE stream.
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// ISO 8601 UTC timestamp with millisecond precision, matching the format
/// used by dictionary rows.
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
