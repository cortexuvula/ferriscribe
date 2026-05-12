//! Tauri commands for the training-corpus curate UI (Phase 2).
//!
//! Backend is GenerationsRepo (in crates/db). These commands wrap
//! list/count/set_status with the AppState + error conversions.

use medical_core::error::{AppError, AppResult};
use medical_db::generations::{Generation, GenerationsRepo};
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CorpusCounts {
    pub candidates: u32,
    pub promoted: u32,
    pub rejected: u32,
    pub excluded: u32,
}

#[derive(Debug, Serialize)]
pub struct GenerationPage {
    pub items: Vec<Generation>,
    pub total: u32,
}

#[tauri::command]
pub async fn training_corpus_counts(
    state: tauri::State<'_, AppState>,
) -> AppResult<CorpusCounts> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    let (c, p, r, e) = GenerationsRepo::count_by_status(&conn)
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(CorpusCounts { candidates: c, promoted: p, rejected: r, excluded: e })
}

#[tauri::command]
pub async fn training_corpus_list(
    state: tauri::State<'_, AppState>,
    status: String,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<GenerationPage> {
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    let (items, total) = GenerationsRepo::list_by_status(
        &conn,
        &status,
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(GenerationPage { items, total })
}

#[tauri::command]
pub async fn training_corpus_set_status(
    state: tauri::State<'_, AppState>,
    id: String,
    new_status: String,
) -> AppResult<()> {
    let id = Uuid::parse_str(&id)
        .map_err(|e| AppError::Other(format!("invalid generation id: {e}")))?;
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    GenerationsRepo::set_corpus_status(&conn, id, &new_status)
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}
