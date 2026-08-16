//! `generate_synopsis` Tauri command — produces a brief synopsis from a recording's SOAP note.

use medical_core::error::{AppError, AppResult};
use medical_processing::document_generator;
use tracing::debug;

use crate::state::AppState;

use super::MAX_SOAP_NOTE_CHARS;
use super::helpers::{
    build_completion_request, load_recording_and_settings, persist_recording, resolve_provider,
};

/// Generate a brief synopsis from a recording's SOAP note.
///
/// The synopsis is returned directly and stored in the recording's metadata
/// (the `Recording` struct does not have a dedicated `synopsis` field).
#[tauri::command]
pub async fn generate_synopsis(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    generate_synopsis_inner(&state, &recording_id).await
}

async fn generate_synopsis_inner(state: &AppState, recording_id: &str) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateSynopsis,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let soap_note = recording
        .soap_note
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::processing(
                "Recording has no SOAP note. Generate a SOAP note first.".to_string(),
            )
        })?;

    if soap_note.len() > MAX_SOAP_NOTE_CHARS {
        return Err(AppError::InvalidInput(format!(
            "SOAP note too large: {} chars, limit is {}",
            soap_note.len(),
            MAX_SOAP_NOTE_CHARS
        )));
    }

    let (system_prompt, user_prompt) = document_generator::build_synopsis_prompt(
        soap_note,
        settings.custom_synopsis_prompt.as_deref(),
        None,
    );

    debug!(
        "generate_synopsis: provider='{}', recording='{}'",
        provider.name(),
        recording_id,
    );

    let model_name = settings.model.clone();
    let request = build_completion_request(
        system_prompt,
        user_prompt,
        model_name.clone(),
        settings.temperature,
        None,
    );

    let generation_start = std::time::Instant::now();
    let response = provider.complete(request).await.map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        // For other errors, keep the existing nicer wrapping.
        _ => AppError::ai_provider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;
    let generation_elapsed = generation_start.elapsed();

    let synopsis_text = response.content;
    if synopsis_text.is_empty() {
        return Err(AppError::ai_provider(
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

    medical_core::types::recording::record_completion_stat(
        &mut recording.metadata,
        "synopsis",
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    );

    persist_recording(&state.db, recording).await?;

    Ok(synopsis_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::build_test_state_with_recording;
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_synopsis_returns_endpoint_offline_when_ai_unreachable() {
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
        let result = generate_synopsis_inner(&state, &recording_id).await;
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

#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
    use super::*;
    use medical_core::types::settings::AppConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_synopsis_records_generation_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Brief synopsis: tension headache, plan follow-up.",
            32,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        // Synopsis generation reads recording.soap_note.
        {
            let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
            let conn = state.db.conn().expect("conn");
            let mut rec =
                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
            rec.soap_note = Some("S: Headache.\nA: Tension headache.\nP: Follow up.".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }

        let synopsis = generate_synopsis_inner(&state, &recording_id)
            .await
            .expect("synopsis generation succeeds");
        assert!(!synopsis.is_empty());

        let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");
        assert_eq!(
            rec.metadata["generation_stats"]["synopsis"]["completion_tokens"],
            serde_json::json!(32)
        );
        // The synopsis text itself stays where it always was.
        assert!(rec.metadata["synopsis"].is_string());
    }
}
