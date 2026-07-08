//! Tauri commands for the per-user spellcheck dictionary.
//!
//! When this client is paired with an office server that advertised a
//! `vocab_port`, dictionary operations route through HTTP to that server
//! (which becomes the canonical source of truth). Otherwise they operate
//! on the local SQLite repo.
//!
//! No word values are emitted to logs — the dictionary may contain
//! patient-context-specific terms.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};

use crate::state::{self, AppState};
use crate::user_dict_remote::UserDictRemote;

/// Returns `Some((conn, bearer))` when this client is paired with an office
/// server that advertised a vocab CRUD API (same port the dictionary API
/// rides on). Commands route through HTTP in that case; otherwise they
/// operate on the local SQLite repo.
fn paired_dict_target() -> Option<(crate::commands::sharing::PairedConnection, String)> {
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

#[tauri::command]
pub async fn user_dict_list(state: tauri::State<'_, AppState>) -> AppResult<Vec<String>> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.list().await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::list(&conn).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

#[tauri::command]
pub async fn user_dict_add(state: tauri::State<'_, AppState>, word: String) -> AppResult<bool> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.add(&word).await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

#[tauri::command]
pub async fn user_dict_remove(state: tauri::State<'_, AppState>, word: String) -> AppResult<bool> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.remove(&word).await;
    }
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}
