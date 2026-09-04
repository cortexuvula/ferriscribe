//! `generate_peer_discussion` Tauri command — turns a recording's transcript
//! into a structured peer discussion note.

use medical_core::error::{AppError, AppResult};
use medical_processing::document_generator;
use medical_processing::peer_discussion::{self, PeerDiscussionPromptConfig};
use tracing::{debug, info};

use crate::state::AppState;

use super::helpers::{
    acquire_generation_lock, build_completion_request, ensure_nonempty_output,
    ensure_prompt_within_cap, fresh_stats_patch, load_recording_and_settings,
    persist_producer_patch, require_transcript, resolve_provider, run_generation_command,
    stream_with_events,
};

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
    // One generation per recording at a time (see acquire_generation_lock).
    let _generation_lock = acquire_generation_lock(&state, &recording_id)?;

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

    let inner = generate_peer_discussion_inner(
        &state,
        Some(&app),
        &recording_id,
        &physician_name,
        &specialty,
        &reason,
        context.as_deref(),
    );
    run_generation_command(
        &app,
        &recording_id,
        "peer_discussion",
        context.as_deref(),
        inner,
    )
    .await
}

async fn generate_peer_discussion_inner(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording_id: &str,
    physician_name: &str,
    specialty: &str,
    reason: &str,
    context: Option<&str>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Same generation-time cap every custom prompt gets — covers configs
    // that arrived via sync.
    ensure_prompt_within_cap(
        settings.custom_peer_discussion_prompt.as_deref(),
        "peer discussion",
    )?;

    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GeneratePeerDiscussion,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let transcript = require_transcript(&recording)?;

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

    let model_name = settings.model.clone();
    let request = build_completion_request(
        system_prompt,
        user_prompt,
        model_name.clone(),
        settings.temperature,
        None,
    );

    let (response, generation_elapsed) =
        stream_with_events(&provider, app, "peer_discussion", recording_id, request).await?;

    // Same plain-text safety net referral/letter get: the system prompt
    // demands plain text, but a model that ignores it must not leak markdown
    // into the stored note and exports.
    let discussion_text = document_generator::strip_markdown(&response.content);
    ensure_nonempty_output(&discussion_text, "peer discussion note")?;

    // Record the stats on the in-memory recording (the patch source), then
    // persist ONLY this document's column plus fresh metadata. A whole-row
    // update on the stale snapshot would revert concurrent column writes
    // (editor saves, another generator's output).
    medical_core::types::recording::record_completion_stat(
        &mut recording.metadata,
        "peer_discussion",
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    );
    let mut metadata_patch = vec![(
        "peer_discussion_context".to_string(),
        serde_json::json!({
            "physician_name": physician_name,
            "specialty": specialty,
            "reason": reason,
        }),
    )];
    metadata_patch.extend(fresh_stats_patch(&recording, "peer_discussion"));

    persist_producer_patch(
        state,
        recording.id,
        medical_db::recordings::ProducerPersist {
            peer_discussion: Some(discussion_text.clone()),
            metadata_patch,
            ..Default::default()
        },
    )
    .await?;

    Ok(discussion_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::{assert_endpoint_offline, build_test_state_with_recording};
    use super::*;
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
            None, // app — no AppHandle in tests
            &recording_id,
            "Smith",
            "Cardiology",
            "chest pain evaluation",
            None, // context
        )
        .await;
        assert_endpoint_offline(result, "Ollama", start);
    }
}

#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
    use super::*;
    use medical_core::types::settings::AppConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_peer_discussion_records_generation_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Discussed the case with cardiology; agreed on outpatient workup.",
            48,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let text = generate_peer_discussion_inner(
            &state,
            None, // app — no AppHandle in tests
            &recording_id,
            "Smith",
            "Cardiology",
            "chest pain evaluation",
            None,
        )
        .await
        .expect("peer discussion generation succeeds");
        assert!(!text.is_empty());

        let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");
        assert_eq!(
            rec.metadata["generation_stats"]["peer_discussion"]["completion_tokens"],
            serde_json::json!(48)
        );
        assert_eq!(
            rec.metadata["generation_stats"]["peer_discussion"]["model"],
            serde_json::json!("llama3")
        );
        assert!(rec.metadata["peer_discussion_context"].is_object());
    }

    /// Regression (generate-pipeline review 2026-09-04): peer discussion
    /// kept referral/letter's "plain text only" prompt but never ran their
    /// markdown-stripping safety net — a model that ignored the instruction
    /// leaked markdown into the stored note.
    #[tokio::test]
    async fn generate_peer_discussion_strips_markdown() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "## Discussion\n\n- **Agreed** on outpatient workup",
            48,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let text = generate_peer_discussion_inner(
            &state,
            None,
            &recording_id,
            "Smith",
            "Cardiology",
            "chest pain evaluation",
            None,
        )
        .await
        .expect("peer discussion generation succeeds");
        assert!(
            !text.contains("##"),
            "headings uppercased, not kept: {text}"
        );
        assert!(!text.contains("**"), "bold markers stripped: {text}");
        assert!(text.contains("Agreed"), "content survives: {text}");
    }
}
