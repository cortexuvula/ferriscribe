//! `generate_synopsis` Tauri command — produces a brief synopsis from a recording's SOAP note.

use medical_core::error::AppResult;
use medical_processing::document_generator;
use tracing::debug;

use crate::state::AppState;

use super::helpers::{
    acquire_generation_lock, build_completion_request, ensure_nonempty_output, fresh_stats_patch,
    load_recording_and_settings, persist_producer_patch, require_soap_note, resolve_provider,
    stream_with_events,
};

/// Generate a brief synopsis from a recording's SOAP note.
///
/// The synopsis is returned directly and stored in the recording's metadata
/// (the `Recording` struct does not have a dedicated `synopsis` field).
#[tauri::command]
pub async fn generate_synopsis(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    // One generation per recording at a time (see acquire_generation_lock).
    let _generation_lock = acquire_generation_lock(&state, &recording_id)?;

    generate_synopsis_inner(&state, Some(&app), &recording_id).await
}

async fn generate_synopsis_inner(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording_id: &str,
) -> AppResult<String> {
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

    let soap_note = require_soap_note(&recording)?;

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

    let (response, generation_elapsed) =
        stream_with_events(&provider, app, "synopsis", recording_id, request).await?;

    let synopsis_text = response.content;
    ensure_nonempty_output(&synopsis_text, "synopsis")?;

    // Record the stats on the in-memory recording (the patch source), then
    // persist ONLY the fresh metadata — the synopsis text and this run's
    // stats. A whole-row update on the stale snapshot would revert
    // concurrent column writes (editor saves, another generator's output).
    medical_core::types::recording::record_completion_stat(
        &mut recording.metadata,
        "synopsis",
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    );
    let mut metadata_patch = vec![(
        "synopsis".to_string(),
        serde_json::Value::String(synopsis_text.clone()),
    )];
    metadata_patch.extend(fresh_stats_patch(&recording, "synopsis"));

    persist_producer_patch(
        state,
        recording.id,
        medical_db::recordings::ProducerPersist {
            metadata_patch,
            ..Default::default()
        },
    )
    .await?;

    Ok(synopsis_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::{assert_endpoint_offline, build_test_state_with_recording};
    use super::*;
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
        let result = generate_synopsis_inner(&state, None, &recording_id).await;
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

        let synopsis = generate_synopsis_inner(&state, None, &recording_id)
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
        assert_eq!(
            rec.metadata["generation_stats"]["synopsis"]["model"],
            serde_json::json!("llama3")
        );
        // The synopsis text itself stays where it always was.
        assert!(rec.metadata["synopsis"].is_string());
    }
}
