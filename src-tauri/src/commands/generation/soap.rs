//! `generate_soap` Tauri command — turns a recording's transcript into a SOAP note.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::PatientContext;
use medical_processing::soap_generator::{self, SoapPromptConfig};
use tauri::Emitter;
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, parse_soap_template,
    patient_context_is_empty, persist_recording, resolve_provider, validate_patient_context,
};
use super::{format_progress_error, GenerationProgress, MAX_CONTEXT_CHARS, MAX_TRANSCRIPT_CHARS};

/// Generate a SOAP note from a recording's transcript.
///
/// Emits `generation-progress` events with `type: "soap"` and statuses
/// `"started"` / `"completed"` / `"failed"`.
#[tauri::command]
pub async fn generate_soap(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    template: Option<String>,
    context: Option<String>,
    patient_context: Option<PatientContext>,
) -> AppResult<String> {
    // Reject oversized user-supplied context up front, before emitting "started"
    // or touching the DB / provider.
    if let Some(ref ctx) = context
        && ctx.len() > MAX_CONTEXT_CHARS {
            return Err(AppError::Other(format!(
                "Context too large: {} chars, limit is {}",
                ctx.len(),
                MAX_CONTEXT_CHARS
            )));
        }
    if let Some(ref pc) = patient_context {
        validate_patient_context(pc)?;
    }

    // Emit: started
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "soap".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_soap_inner(
        &state,
        &recording_id,
        template.as_deref(),
        context.as_deref(),
        patient_context.as_ref(),
    )
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "soap".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "soap".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}

#[instrument(skip(state, context, patient_context), fields(recording_id = %recording_id))]
async fn generate_soap_inner(
    state: &AppState,
    recording_id: &str,
    template: Option<&str>,
    context: Option<&str>,
    patient_context: Option<&PatientContext>,
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    {
        let config = {
            let db = Arc::clone(&state.db);
            tokio::task::spawn_blocking(move || -> medical_core::error::AppResult<medical_core::types::settings::AppConfig> {
                let conn = db.conn().map_err(|e| medical_core::error::AppError::Database(e.to_string()))?;
                let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn)
                    .map_err(|e| medical_core::error::AppError::Database(e.to_string()))?;
                cfg.migrate();
                Ok(cfg)
            })
            .await
            .map_err(|e| medical_core::error::AppError::Other(format!("preflight config load join error: {e}")))??
        };
        medical_core::preflight::preflight_for_command(
            medical_core::preflight::CommandKind::GenerateSoap,
            &config,
        )
        .await?;
    }

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let transcript = recording
        .transcript
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            AppError::Processing("Recording has no transcript. Run transcription first.".to_string())
        })?;

    if transcript.len() > MAX_TRANSCRIPT_CHARS {
        return Err(AppError::Other(format!(
            "Transcript too large: {} chars, limit is {}",
            transcript.len(),
            MAX_TRANSCRIPT_CHARS
        )));
    }

    info!(
        provider = %provider.name(),
        model = %settings.model,
        template = template.unwrap_or("follow_up"),
        transcript_len = transcript.len(),
        context_len = context.map(|c| c.len()).unwrap_or(0),
        patient_context_present = patient_context.is_some(),
        "Generating SOAP note"
    );

    // Build prompts with full config
    let soap_template = template.map(parse_soap_template).unwrap_or_default();
    let model_name = settings.model.clone();
    let config = SoapPromptConfig {
        template: soap_template,
        icd_version: settings.icd_version,
        custom_prompt: settings.custom_soap_prompt,
    };

    let system_prompt = soap_generator::build_soap_prompt(&config);
    let user_prompt = soap_generator::build_user_prompt(transcript, context, patient_context);

    debug!(
        "generate_soap: provider='{}', recording='{}', context_len={}, patient_context_present={}",
        provider.name(),
        recording_id,
        context.map(|c| c.len()).unwrap_or(0),
        patient_context.is_some(),
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
        .map_err(|e| match e {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            AppError::EndpointOffline { .. } => e,
            // For other errors, keep the existing nicer wrapping.
            _ => AppError::AiProvider(format!(
                "AI completion failed: {}",
                crate::commands::unwrap_app_error_message(e)
            )),
        })?;

    let raw_soap = response.content;
    if raw_soap.is_empty() {
        error!(
            provider = %provider.name(),
            model = %model_name,
            "AI returned an empty SOAP note"
        );
        return Err(AppError::AiProvider(format!(
            "AI returned an empty SOAP note (provider: {}, model: {}). \
             Check that the model is loaded and responding.",
            provider.name(),
            model_name,
        )));
    }

    info!(
        raw_len = raw_soap.len(),
        "AI completion received, post-processing"
    );

    // Post-process: strip markdown, fix paragraph formatting
    let soap_text = soap_generator::postprocess_soap(&raw_soap);

    // ── Training-corpus capture (Task 6a) ────────────────────────────────
    // Gated on AppConfig.capture_for_training (default: false). Failure
    // must never break the user's workflow — log at warn and continue.
    let recording_uuid = Uuid::parse_str(recording_id)
        .ok(); // already validated by load_recording_and_settings; unwrap safe

    let capture_generation_id: Option<Uuid> = if let Some(rec_uuid) = recording_uuid {
        match state.db.conn() {
            Ok(conn) => {
                let cfg = medical_db::settings::SettingsRepo::load_config(&conn)
                    .unwrap_or_default();
                if cfg.capture_for_training {
                    let context_blob = serde_json::json!({
                        "context": context,
                        "patient_context": patient_context,
                    });
                    let context_json = context_blob.to_string();
                    let provider_name = provider.name().to_string();
                    let insert = medical_db::generations::GenerationInsert {
                        recording_id: rec_uuid,
                        output_type: "soap",
                        ai_provider: &provider_name,
                        ai_model: &model_name,
                        prompt_template_name: template,
                        input_transcript: transcript,
                        input_context_json: Some(context_json.as_str()),
                        draft_text: &soap_text,
                    };
                    match medical_db::generations::GenerationsRepo::record_generation(&conn, insert) {
                        Ok(g) => {
                            tracing::debug!(generation_id = %g.id, "captured SOAP generation for training corpus");
                            Some(g.id)
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "training-corpus capture failed; continuing");
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "training-corpus capture: could not open DB connection; continuing");
                None
            }
        }
    } else {
        None
    };

    // Save context to recording metadata for future reference.
    if recording.metadata.is_null() {
        recording.metadata = serde_json::json!({});
    }
    if let Some(obj) = recording.metadata.as_object_mut() {
        if let Some(ctx) = context
            && !ctx.is_empty() {
                obj.insert("context".to_string(), serde_json::Value::String(ctx.to_string()));
            }
        if let Some(pc) = patient_context
            && !patient_context_is_empty(pc) {
                obj.insert(
                    "patient_context".to_string(),
                    serde_json::to_value(pc)
                        .unwrap_or(serde_json::Value::Null),
                );
            }
    }

    // Persist to DB (on blocking thread)
    recording.soap_note = Some(soap_text.clone());
    persist_recording(&state.db, recording).await?;

    // ── Training-corpus finalize (Task 6b) ───────────────────────────────
    // Mirror the saved text into the generations row's final_text. Only
    // runs when capture actually inserted a row above — gating on
    // capture_generation_id avoids an unnecessary DB query (and the
    // SettingsRepo / GenerationsRepo round trips) on every SOAP
    // generation for users who haven't opted into capture.
    if capture_generation_id.is_some() {
        let rec_uuid = recording_uuid
            .expect("capture_generation_id Some implies recording_uuid Some");
        match state.db.conn() {
            Ok(conn) => {
                match medical_db::generations::GenerationsRepo::update_final_text(
                    &conn,
                    rec_uuid,
                    "soap",
                    &soap_text,
                ) {
                    Ok(Some(g)) => {
                        tracing::debug!(generation_id = %g.id, "updated final_text on generations row");
                        spawn_edit_distance_task(
                            Arc::clone(&state.db),
                            g.id,
                            g.draft_text.clone(),
                            soap_text.clone(),
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "training-corpus finalize failed; continuing");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "training-corpus finalize: could not open DB connection; continuing");
            }
        }
    }

    Ok(soap_text)
}

#[cfg(test)]
mod preflight_tests {
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::recording::{ProcessingStatus, Recording};
    use medical_core::types::settings::AppConfig;
    use medical_db::recordings::RecordingsRepo;
    use medical_db::settings::SettingsRepo;
    use tokio::sync::{Mutex, RwLock};

    /// Build a minimal `AppState` backed by an in-memory DB.
    /// The DB is pre-loaded with `config` and a recording whose
    /// transcript is `transcript_text`.
    /// Returns `(state, recording_id_string)`.
    async fn build_test_state_with_recording(
        config: AppConfig,
        transcript_text: &str,
    ) -> (AppState, String) {
        // ── Database ────────────────────────────────────────────────────────
        let db = Arc::new(medical_db::Database::open_in_memory().expect("open in-memory db"));

        // Save the config (AI provider endpoint) so load_recording_and_settings
        // picks it up, and preflight_for_command reads the right host/port.
        {
            let conn = db.conn().expect("conn");
            SettingsRepo::save_config(&conn, &config).expect("save_config");
        }

        // Insert a recording with the given transcript.
        let recording_id = {
            use std::path::PathBuf;
            let id = uuid::Uuid::new_v4();
            let mut rec = Recording::new(
                format!("{}.wav", id),
                PathBuf::from(format!("/tmp/{}.wav", id)),
            );
            rec.id = id;
            rec.status = ProcessingStatus::Pending;
            rec.transcript = Some(transcript_text.to_string());
            let conn = db.conn().expect("conn");
            RecordingsRepo::insert(&conn, &rec).expect("insert recording");
            id
        };

        // ── AI provider registry ─────────────────────────────────────────────
        // Register Ollama pointing at the configured (unrouteable) host.
        // The provider is registered so resolve_provider doesn't fail, but
        // pre-flight fires before provider.complete() is ever invoked.
        let mut registry = medical_ai_providers::ProviderRegistry::new();
        let ollama_host = if config.ollama_host.is_empty() {
            "localhost"
        } else {
            config.ollama_host.as_str()
        };
        let ollama_url = format!("http://{}:{}", ollama_host, config.ollama_port);
        if let Ok(p) = medical_ai_providers::ollama::OllamaProvider::new_with_endpoint(
            Some(&ollama_url),
            None,
            medical_ai_providers::http_client::RetryConfig::default(),
            None,
        ) {
            registry.register(Arc::new(p) as Arc<dyn medical_core::traits::AiProvider>);
            registry.set_active(&config.ai_provider);
        }

        // ── Key storage ──────────────────────────────────────────────────────
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join("config");
        let keys = medical_security::key_storage::KeyStorage::open(&config_dir)
            .expect("KeyStorage::open");
        // Keep `tmp` alive for the duration of the state — we do this by
        // moving it into a Box and leaking it (acceptable in tests).
        std::mem::forget(tmp);

        // ── RAG subsystem ────────────────────────────────────────────────────
        let embedding_generator = Arc::new(
            medical_rag::embeddings::EmbeddingGenerator::new_ollama(None, None)
        );
        let vector_store = Arc::new(medical_rag::vector_store::VectorStore::new(Arc::clone(&db)));
        let bm25_search = Arc::new(medical_rag::bm25::Bm25Search::new(Arc::clone(&db)));
        let graph_search = Arc::new(medical_rag::graph_search::GraphSearch::new(Arc::clone(&db)));
        let ingestion = Arc::new(medical_rag::ingestion::IngestionPipeline::new(
            Arc::clone(&embedding_generator),
            Arc::clone(&vector_store),
            Arc::clone(&graph_search),
        ));

        // ── Agent orchestrator ───────────────────────────────────────────────
        let tool_registry = medical_agents::tools::ToolRegistry::with_defaults();
        let orchestrator = Arc::new(medical_agents::orchestrator::AgentOrchestrator::new(tool_registry));

        // ── HTTP client ──────────────────────────────────────────────────────
        let http_client = Arc::new(reqwest::Client::new());

        let state = AppState {
            db,
            keys: Arc::new(keys),
            data_dir: std::path::PathBuf::from("/tmp/test-data"),
            recording_active: Arc::new(Mutex::new(false)),
            ai_providers: Arc::new(Mutex::new(registry)),
            stt_providers: Arc::new(Mutex::new(None)),
            orchestrator,
            capture_handle: Arc::new(std::sync::Mutex::new(crate::state::SendCaptureHandle(None))),
            current_recording: Arc::new(std::sync::Mutex::new(None)),
            pipeline_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            embedding_generator,
            vector_store,
            bm25_search,
            graph_search,
            ingestion,
            sharing: Arc::new(RwLock::new(None)),
            vocab_api: RwLock::new(None),
            ollama_provider: RwLock::new(None),
            lmstudio_provider: RwLock::new(None),
            remote_stt_provider: RwLock::new(None),
            http_client,
        };

        (state, recording_id.to_string())
    }

    #[tokio::test]
    async fn generate_soap_returns_endpoint_offline_when_ai_unreachable() {
        // 192.0.2.1 is RFC 5737 TEST-NET-1 — guaranteed unrouteable, so
        // the probe times out within PROBE_TIMEOUT (3s).
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "192.0.2.1".to_string();
        config.ollama_port = 11434;
        config.ai_model = "llama3".to_string();

        let (state, recording_id) = build_test_state_with_recording(
            config,
            "Patient reports headache and fatigue.",
        )
        .await;

        let start = std::time::Instant::now();
        let result = generate_soap_inner(
            &state,
            &recording_id,
            None, // template
            None, // context
            None, // patient_context
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
                // 192.0.2.1 is unrouteable — Timeout is the expected outcome.
                // ConnectionRefused is also acceptable if the OS responds fast.
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

        // Pre-flight must short-circuit BEFORE the real call: ~3s probe ceiling
        // plus some overhead, much less than the real call's timeout.
        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "should have short-circuited at ~3s; took {elapsed:?}"
        );
    }
}

/// Spawn a blocking task that computes word-level Levenshtein between the
/// draft and the final (saved) text, then writes the result back to the
/// `generations` row. Best-effort: failures log at warn and are discarded.
///
/// `pub(crate)` so the edit-save command in `recordings_edit` can reuse it.
pub(crate) fn spawn_edit_distance_task(
    db: Arc<medical_db::Database>,
    generation_id: Uuid,
    draft: String,
    final_text: String,
) {
    tokio::task::spawn_blocking(move || {
        let (distance, ratio) =
            medical_processing::edit_distance::word_edit_distance(&draft, &final_text);
        match db.conn() {
            Ok(conn) => {
                if let Err(e) = medical_db::generations::GenerationsRepo::set_edit_distance(
                    &conn,
                    generation_id,
                    distance as i64,
                    ratio,
                ) {
                    tracing::warn!(
                        error = %e,
                        generation_id = %generation_id,
                        "set_edit_distance failed"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    generation_id = %generation_id,
                    "edit-distance task could not open DB connection"
                );
            }
        }
    });
}
