//! Tauri command for the corpus-export pipeline (Phase 3).

use medical_core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::corpus_export::{self, ExportOptions, RedactionStrictness};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    pub output_dir: String,
    pub base_model_filter: Vec<String>,
    pub redaction_strictness: String, // 'standard' | 'aggressive'
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub corpus_dir: String,
    pub pairs_written: u32,
    pub warning_count: u32,
}

#[tauri::command]
pub async fn training_corpus_export(
    state: tauri::State<'_, AppState>,
    req: ExportRequest,
) -> AppResult<ExportResponse> {
    let strictness = match req.redaction_strictness.as_str() {
        "aggressive" => RedactionStrictness::Aggressive,
        _ => RedactionStrictness::Standard,
    };
    let opts = ExportOptions {
        output_dir: PathBuf::from(req.output_dir),
        base_model_filter: req.base_model_filter,
        redaction_strictness: strictness,
        ferri_scribe_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    // The pipeline is sync (file I/O + regex); spawn_blocking
    // keeps the runtime responsive.
    let db = std::sync::Arc::clone(&state.db);
    let result = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        corpus_export::export(&conn, opts)
    })
    .await
    .map_err(|e| AppError::Other(format!("export task join: {e}")))?
    .map_err(|e| AppError::Other(format!("export failed: {e}")))?;

    Ok(ExportResponse {
        corpus_dir: result.corpus_dir.to_string_lossy().to_string(),
        pairs_written: result.pairs_written,
        warning_count: result.warnings.len() as u32,
    })
}
