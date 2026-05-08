//! `generate_synopsis` Tauri command — produces a brief synopsis from a recording's SOAP note.

use medical_core::error::{AppError, AppResult};
use medical_processing::document_generator;
use tracing::debug;

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, persist_recording, resolve_provider,
};
use super::MAX_SOAP_NOTE_CHARS;

/// Generate a brief synopsis from a recording's SOAP note.
///
/// The synopsis is returned directly and stored in the recording's metadata
/// (the `Recording` struct does not have a dedicated `synopsis` field).
#[tauri::command]
pub async fn generate_synopsis(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, &recording_id).await?;
    let provider = resolve_provider(&state, &settings.ai_provider).await?;

    let soap_note = recording
        .soap_note
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::Processing(
                "Recording has no SOAP note. Generate a SOAP note first.".to_string(),
            )
        })?;

    if soap_note.len() > MAX_SOAP_NOTE_CHARS {
        return Err(AppError::Other(format!(
            "SOAP note too large: {} chars, limit is {}",
            soap_note.len(),
            MAX_SOAP_NOTE_CHARS
        )));
    }

    let (system_prompt, user_prompt) = document_generator::build_synopsis_prompt(
        soap_note,
        settings.custom_synopsis_prompt.as_deref(),
    );

    debug!(
        "generate_synopsis: provider='{}', recording='{}'",
        provider.name(),
        recording_id,
    );

    let request = build_completion_request(
        system_prompt,
        user_prompt,
        settings.model,
        settings.temperature,
        None,
    );

    let response = provider
        .complete(request)
        .await
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", crate::commands::unwrap_app_error_message(e))))?;

    let synopsis_text = response.content;
    if synopsis_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty synopsis.".to_string(),
        ));
    }

    // Store synopsis in the metadata JSON object.
    if recording.metadata.is_null() {
        recording.metadata = serde_json::json!({});
    }
    if let Some(obj) = recording.metadata.as_object_mut() {
        obj.insert(
            "synopsis".to_string(),
            serde_json::Value::String(synopsis_text.clone()),
        );
    }
    persist_recording(&state.db, recording).await?;

    Ok(synopsis_text)
}
