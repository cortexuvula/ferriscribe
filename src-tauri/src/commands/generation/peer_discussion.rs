//! `generate_peer_discussion` Tauri command — turns a recording's transcript
//! into a structured peer discussion note.

use medical_core::error::{AppError, AppResult};
use medical_processing::peer_discussion::{self, PeerDiscussionPromptConfig};
use tauri::Emitter;
use tracing::{debug, info};

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, persist_recording, resolve_provider,
};
use super::{GenerationProgress, MAX_CONTEXT_CHARS, MAX_TRANSCRIPT_CHARS, format_progress_error};

/// Generate a peer discussion note from a recording's transcript.
///
/// Emits `generation-progress` events with `type: "peer_discussion"` and
/// statuses `"started"` / `"completed"` / `"failed"`.
#[tauri::command]
pub async fn generate_peer_discussion(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    physician_name: String,
    specialty: String,
    reason: String,
    context: Option<String>,
) -> AppResult<String> {
    if physician_name.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Physician name is required.".to_string(),
        ));
    }
    if reason.trim().is_empty() {
        return Err(AppError::Other(
            "Reason for discussion is required.".to_string(),
        ));
    }
    if let Some(ref ctx) = context
        && ctx.len() > MAX_CONTEXT_CHARS
    {
        return Err(AppError::InvalidInput(format!(
            "Context too large: {} chars, limit is {}",
            ctx.len(),
            MAX_CONTEXT_CHARS
        )));
    }

    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "peer_discussion".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_peer_discussion_inner(
        &state,
        &recording_id,
        &physician_name,
        &specialty,
        &reason,
        context.as_deref(),
    )
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "peer_discussion".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "peer_discussion".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}

async fn generate_peer_discussion_inner(
    state: &AppState,
    recording_id: &str,
    physician_name: &str,
    specialty: &str,
    reason: &str,
    context: Option<&str>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GeneratePeerDiscussion,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let transcript = recording
        .transcript
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            AppError::Processing(
                "Recording has no transcript. Run transcription first.".to_string(),
            )
        })?;

    if transcript.len() > MAX_TRANSCRIPT_CHARS {
        return Err(AppError::InvalidInput(format!(
            "Transcript too large: {} chars, limit is {}",
            transcript.len(),
            MAX_TRANSCRIPT_CHARS
        )));
    }

    // PHI guard: physician_name/specialty are provider PHI — log only
    // structural metadata (AGENTS.md line 6).
    info!(
        provider = %provider.name(),
        model = %settings.model,
        transcript_len = transcript.len(),
        "Generating peer discussion note"
    );

    let prompt_config = PeerDiscussionPromptConfig {
        physician_name: physician_name.to_string(),
        specialty: specialty.to_string(),
        reason: reason.to_string(),
        custom_prompt: settings.custom_peer_discussion_prompt.clone(),
    };

    let system_prompt = peer_discussion::build_peer_discussion_prompt(&prompt_config);
    let user_prompt =
        peer_discussion::build_user_prompt(transcript, physician_name, specialty, reason, context);

    debug!(
        "generate_peer_discussion: provider='{}', recording='{}'",
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

    let response = provider.complete(request).await.map_err(|e| match e {
        AppError::EndpointOffline { .. } => e,
        _ => AppError::AiProvider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;

    let discussion_text = response.content;
    if discussion_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty peer discussion note.".to_string(),
        ));
    }

    if recording.metadata.is_null() {
        recording.metadata = serde_json::json!({});
    }
    if let Some(obj) = recording.metadata.as_object_mut() {
        obj.insert(
            "peer_discussion_context".to_string(),
            serde_json::json!({
                "physician_name": physician_name,
                "specialty": specialty,
                "reason": reason,
            }),
        );
    }

    recording.peer_discussion = Some(discussion_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(discussion_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::build_test_state_with_recording;
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_peer_discussion_returns_endpoint_offline_when_ai_unreachable() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "192.0.2.1".to_string();
        config.ollama_port = 11434;
        config.ai_model = "llama3".to_string();

        let (state, recording_id) =
            build_test_state_with_recording(config, "Patient reports headache and fatigue.").await;

        let start = std::time::Instant::now();
        let result = generate_peer_discussion_inner(
            &state,
            &recording_id,
            "Smith",
            "Cardiology",
            "chest pain evaluation",
            None, // context
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
