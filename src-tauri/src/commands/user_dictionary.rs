//! Tauri commands for the per-user spellcheck dictionary.
//!
//! Backend is [`medical_db::user_dictionary::UserDictionaryRepo`]. These
//! commands surface list/add/remove for the in-app spellchecker so the
//! frontend can manage the "accepted spellings" wordlist.
//!
//! No word values are emitted to logs — the dictionary may contain
//! patient-context-specific terms.

use medical_core::error::{AppError, AppResult};

use crate::state::AppState;

#[tauri::command]
pub async fn user_dict_list(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<String>> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::list(&conn)
        .map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_add(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word)
        .map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_remove(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
        .map_err(|e| AppError::Database(e.to_string()))
}
