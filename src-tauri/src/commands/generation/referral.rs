//! `generate_referral` Tauri command — turns a recording's SOAP note into a referral.

use medical_core::error::{AppError, AppResult};
use medical_processing::document_generator;
use tauri::Emitter;
use tracing::debug;

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, persist_recording, resolve_provider,
};
use super::{format_progress_error, GenerationProgress, MAX_SOAP_NOTE_CHARS};

/// Generate a referral letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "referral"`.
#[tauri::command]
pub async fn generate_referral(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    recipient_type: Option<String>,
    urgency: Option<String>,
) -> AppResult<String> {
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "referral".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_referral_inner(
        &state,
        &recording_id,
        recipient_type.as_deref(),
        urgency.as_deref(),
    )
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "referral".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "referral".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}

async fn generate_referral_inner(
    state: &AppState,
    recording_id: &str,
    recipient_type: Option<&str>,
    urgency: Option<&str>,
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, recording_id).await?;
    let provider = resolve_provider(state, &settings.ai_provider).await?;

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

    let recipient = recipient_type.unwrap_or("Specialist");
    let urg = urgency.unwrap_or("routine");

    let (system_prompt, user_prompt) = document_generator::build_referral_prompt(
        soap_note,
        recipient,
        urg,
        settings.custom_referral_prompt.as_deref(),
    );

    debug!(
        "generate_referral: provider='{}', recording='{}', recipient='{}', urgency='{}'",
        provider.name(),
        recording_id,
        recipient,
        urg,
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

    let referral_text = response.content;
    if referral_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty referral letter.".to_string(),
        ));
    }

    // Persist to DB (on blocking thread)
    recording.referral = Some(referral_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(referral_text)
}
