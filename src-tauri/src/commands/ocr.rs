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
    /// Maximum number of files accepted in a single OCR batch. Guards against
    /// unbounded processing time from accidental large drops.
    const MAX_BATCH_FILES: usize = 25;

    // Filter empty/whitespace paths.
    let file_paths: Vec<String> = file_paths
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if file_paths.is_empty() {
        return Ok(vec![]);
    }
    if file_paths.len() > MAX_BATCH_FILES {
        return Err(AppError::Other(format!(
            "Too many files: {} (limit {MAX_BATCH_FILES}). Process in smaller groups.",
            file_paths.len()
        )));
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

    // If the batch contains any PDFs, make sure the pdfium renderer (used to
    // OCR scanned/image-only PDFs) is downloaded + bound. This is lazy + cached:
    // the ~7 MB library is fetched into the app data dir on first use and reused
    // thereafter. Non-PDF batches skip this entirely.
    if file_paths
        .iter()
        .any(|p| p.to_lowercase().ends_with(".pdf"))
        && let Err(e) = ocr::ensure_pdfium_initialized(&state.data_dir).await
    {
        tracing::warn!(error = %e, "pdfium ensure failed; scanned-PDF OCR will report unavailable");
        // Proceed — each scanned PDF will surface PDFIUM_UNAVAILABLE_MSG.
    }

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
