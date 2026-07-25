//! Per-user spellcheck dictionary handlers for the `/v1/user-dictionary`
//! routes.
//!
//! Reads/writes hit `medical_db::user_dictionary::UserDictionaryRepo`
//! against the office server's local SQLite DB. Same bearer auth +
//! spawn_blocking pattern as the vocab handlers. No PHI in logs.

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
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
    let added =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api add failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, added, "dict_api: add");
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
    let removed =
        tokio::task::spawn_blocking(move || -> Result<bool, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
                .map_err(medical_core::error::AppError::from)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!("dict_api remove failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    info!(word_len, removed, "dict_api: remove");
    Ok(Json(removed))
}
