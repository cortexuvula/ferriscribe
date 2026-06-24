//! `generate_letter` Tauri command — turns a recording's SOAP note into a patient letter.

use medical_core::error::{AppError, AppResult};
use medical_db::LetterAudiencesRepo;
use medical_processing::document_generator::{self, LetterAudienceContext};
use tauri::Emitter;
use tracing::debug;
use uuid::Uuid;

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, persist_recording, resolve_provider,
};
use super::{GenerationProgress, MAX_SOAP_NOTE_CHARS, format_progress_error};

/// Generate a patient letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "letter"`.
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
    audience_id: Option<Uuid>,
) -> AppResult<String> {
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "letter".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_letter_inner(
        &state,
        &recording_id,
        letter_type.as_deref(),
        audience_id.as_ref(),
    )
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "letter".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "letter".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}

async fn generate_letter_inner(
    state: &AppState,
    recording_id: &str,
    letter_type: Option<&str>,
    audience_id: Option<&Uuid>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // If an audience_id is provided, fetch it from the DB and convert to context.
    let audience_context: Option<LetterAudienceContext> = match audience_id {
        Some(id) => {
            let conn = state
                .db
                .conn()
                .map_err(|e| AppError::Database(e.to_string()))?;
            let audience = LetterAudiencesRepo::get_by_id(&conn, id)
                .map_err(|e| AppError::Database(e.to_string()))?;
            Some(LetterAudienceContext {
                name: audience.name,
                system_prompt: audience.system_prompt,
                user_template: audience.user_template,
            })
        }
        None => None,
    };

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateLetter,
        &config,
    )
    .await?;

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

    let ltype = letter_type.unwrap_or("follow-up");

    let (system_prompt, user_prompt) = document_generator::build_letter_prompt(
        soap_note,
        ltype,
        audience_context.as_ref(),
        settings.custom_letter_prompt.as_deref(),
    );

    debug!(
        "generate_letter: provider='{}', recording='{}', letter_type='{}'",
        provider.name(),
        recording_id,
        ltype,
    );

    let request = build_completion_request(
        system_prompt,
        user_prompt,
        settings.model,
        settings.temperature,
        None,
    );

    let response = provider.complete(request).await.map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        // For other errors, keep the existing nicer wrapping.
        _ => AppError::AiProvider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;

    let letter_text = medical_processing::document_generator::strip_markdown(&response.content);
    if letter_text.trim().is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty letter.".to_string(),
        ));
    }

    // Persist to DB (on blocking thread)
    recording.letter = Some(letter_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(letter_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::build_test_state_with_recording;
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_letter_returns_endpoint_offline_when_ai_unreachable() {
        // 192.0.2.1 is RFC 5737 TEST-NET-1 — guaranteed unrouteable, so
        // the probe times out within PROBE_TIMEOUT (3s).
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "192.0.2.1".to_string();
        config.ollama_port = 11434;
        config.ai_model = "llama3".to_string();

        let (state, recording_id) =
            build_test_state_with_recording(config, "Patient reports headache and fatigue.").await;

        let start = std::time::Instant::now();
        let result = generate_letter_inner(
            &state,
            &recording_id,
            None, // letter_type
            None, // audience_id
        )
        .await;
        let elapsed = start.elapsed();

        let err = result.expect_err("must fail with offline error");
        match err {
            AppError::EndpointOffline {
                service,
                reason,
                provider_name,
                ..
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(provider_name, "Ollama");
                assert!(
                    matches!(
                        reason,
                        OfflineReason::ConnectionRefused | OfflineReason::Timeout
                    ),
                    "expected ConnectionRefused or Timeout, got {reason:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }

        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "should have short-circuited at ~3s; took {elapsed:?}"
        );
    }
}
