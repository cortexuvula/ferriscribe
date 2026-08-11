//! `generate_letter_from_document` Tauri command — the standalone Letter Writer.
//!
//! Unlike the other generation commands, this one is not tied to a recording:
//! it takes already-extracted document text (typically OCR'd by the existing
//! `ocr_documents` pipeline) plus a few optional fields and freeform writer's
//! instructions, and returns a drafted letter. The result is ephemeral — it is
//! not persisted to the DB — so a future "letters" table can be layered on
//! without changing this command's signature.

use medical_core::error::{AppError, AppResult};
use medical_processing::document_generator;
use tracing::debug;

use crate::state::AppState;

use super::MAX_DOCUMENT_CHARS;
use super::helpers::{build_completion_request, load_config, resolve_provider};

/// Draft a letter from a source document (e.g. OCR'd text) plus optional
/// structured fields and freeform writer's instructions.
///
/// Standalone: no recording is required and nothing is persisted. The OCR step
/// itself is handled by the existing `ocr_documents` command; this command only
/// does the text → letter generation, using the standard AI model
/// (`config.ai_model`) — the OCR/vision model is not used here.
#[tauri::command]
pub async fn generate_letter_from_document(
    state: tauri::State<'_, AppState>,
    document_text: String,
    recipient: Option<String>,
    letter_type: Option<String>,
    tone: Option<String>,
    re_line: Option<String>,
    user_instructions: Option<String>,
) -> AppResult<String> {
    generate_letter_from_document_inner(
        &state,
        document_text,
        recipient,
        letter_type,
        tone,
        re_line,
        user_instructions,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn generate_letter_from_document_inner(
    state: &AppState,
    document_text: String,
    recipient: Option<String>,
    letter_type: Option<String>,
    tone: Option<String>,
    re_line: Option<String>,
    user_instructions: Option<String>,
) -> AppResult<String> {
    let document_text = document_text.trim().to_string();
    if document_text.is_empty() {
        return Err(AppError::InvalidInput(
            "Document text is empty. OCR or paste a document first.".to_string(),
        ));
    }
    if document_text.len() > MAX_DOCUMENT_CHARS {
        return Err(AppError::InvalidInput(format!(
            "Document too large: {} chars, limit is {}",
            document_text.len(),
            MAX_DOCUMENT_CHARS
        )));
    }

    let config = load_config(&state.db).await?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Reuses the GenerateLetter variant — same AI provider/model tier — so no
    // new CommandKind is needed.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateLetter,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &config.ai_provider).await?;

    let (system_prompt, user_prompt) = document_generator::build_letter_from_document_prompt(
        &document_text,
        recipient.as_deref(),
        letter_type.as_deref(),
        tone.as_deref(),
        re_line.as_deref(),
        user_instructions.as_deref(),
        config.custom_letter_writer_prompt.as_deref(),
    );

    // Log counts/lengths only — never document or letter content (PHI).
    debug!(
        provider = %provider.name(),
        document_chars = document_text.len(),
        has_instructions = user_instructions.as_deref().is_some_and(|s| !s.trim().is_empty()),
        "generate_letter_from_document",
    );

    let request = build_completion_request(
        system_prompt,
        user_prompt,
        config.ai_model,
        config.temperature,
        None,
    );

    let response = provider.complete(request).await.map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        _ => AppError::AiProvider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;

    let letter = document_generator::strip_markdown(&response.content);
    if letter.trim().is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty letter.".to_string(),
        ));
    }

    // Ephemeral: no DB write. A future letters table can be added without
    // changing this command's signature.
    Ok(letter)
}
