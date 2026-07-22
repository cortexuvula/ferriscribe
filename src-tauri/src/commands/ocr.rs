//! Tauri command for OCR document processing.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_processing::ocr::{self, OcrPageResult};

use crate::commands::generation;
use crate::state::AppState;

/// Extract text from document files (PDFs, images, text files).
///
/// Files are classified by extension: text files are read directly, images
/// are sent to the configured vision model, PDFs are text-extracted.
///
/// The OCR model is taken from `config.ocr_model`, falling back to `ai_model`.
#[tauri::command]
pub async fn ocr_documents(
    state: tauri::State<'_, AppState>,
    file_paths: Vec<String>,
) -> AppResult<Vec<OcrPageResult>> {
    if file_paths.is_empty() {
        return Ok(vec![]);
    }

    // Load config to get the OCR model name.
    let db = Arc::clone(&state.db);
    let (ocr_model, provider_name) = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)?;
        config.migrate();
        let model = config
            .ocr_model
            .clone()
            .filter(|m| !m.is_empty())
            .or_else(|| Some(config.ai_model.clone()))
            .unwrap_or_default();
        let provider = config.ai_provider.clone();
        Ok::<_, AppError>((model, provider))
    })
    .await
    .map_err(crate::commands::join_err)??;

    let provider = generation::resolve_provider(&state, &provider_name).await?;

    let results = ocr::extract_text(&file_paths, &ocr_model, provider)
        .await
        .map_err(|e| AppError::Other(format!("OCR failed: {e}")))?;

    tracing::info!(
        file_count = file_paths.len(),
        success_count = results.len(),
        "ocr_documents complete"
    );

    Ok(results)
}
