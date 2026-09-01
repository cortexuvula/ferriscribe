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
    build_completion_request, load_recording_and_settings, patient_context_is_empty,
    persist_recording, resolve_provider, resolve_soap_template, validate_patient_context,
};
use super::{GenerationProgress, MAX_CONTEXT_CHARS, MAX_TRANSCRIPT_CHARS, format_progress_error};

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
        && ctx.len() > MAX_CONTEXT_CHARS
    {
        return Err(AppError::InvalidInput(format!(
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
            progress: None,
        },
    );

    let result = generate_soap_inner(
        &state,
        Some(&app),
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
                    progress: None,
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
                    progress: None,
                },
            );
        }
    }

    result
}

#[instrument(skip(state, app, context, patient_context), fields(recording_id = %recording_id))]
async fn generate_soap_inner(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording_id: &str,
    template: Option<&str>,
    context: Option<&str>,
    patient_context: Option<&PatientContext>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateSoap,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let transcript = recording
        .transcript
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            AppError::processing(
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

    // Explicit template wins; otherwise the stored preference from
    // AppConfig. Resolved here so every caller path (pipeline, Generate
    // tab, Regenerate) honors the stored setting — previously a call with
    // no template silently used FollowUp.
    let soap_template = resolve_soap_template(template, &config);

    info!(
        provider = %provider.name(),
        model = %settings.model,
        template = ?soap_template,
        transcript_len = transcript.len(),
        context_len = context.map(|c| c.len()).unwrap_or(0),
        patient_context_present = patient_context.is_some(),
        "Generating SOAP note"
    );

    // Build prompts with full config
    let model_name = settings.model.clone();

    // Select BC MSP ICD-9 candidates relevant to this visit. Only
    // computed for ICD-9 / both modes; empty for ICD-10-only. The
    // selector reads the transcript + context + patient conditions so
    // the prompt surfaces the most likely billable codes.
    let icd9_candidates = match settings.icd_version {
        medical_core::types::settings::IcdVersion::Icd9
        | medical_core::types::settings::IcdVersion::Both => {
            medical_processing::soap_generator::icd_selector::select_icd9_candidates(
                transcript,
                context,
                patient_context,
            )
        }
        medical_core::types::settings::IcdVersion::Icd10 => Vec::new(),
    };
    info!(
        icd9_candidates_selected = icd9_candidates.len(),
        "ICD-9 candidate selection complete"
    );

    let prompt_config = SoapPromptConfig {
        template: soap_template,
        icd_version: settings.icd_version,
        custom_prompt: settings.custom_soap_prompt,
        icd9_candidates,
    };

    let system_prompt = soap_generator::build_soap_prompt(&prompt_config);
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

    let generation_start = std::time::Instant::now();
    let response = super::stream::stream_to_completion(
        &provider,
        |stats| {
            if let Some(app) = app {
                let _ = app.emit(
                    "generation-progress",
                    GenerationProgress {
                        doc_type: "soap".into(),
                        status: "generating".into(),
                        recording_id: recording_id.to_string(),
                        progress: Some(*stats),
                    },
                );
            }
        },
        request,
    )
    .await
    .map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        // For other errors, keep the existing nicer wrapping.
        _ => AppError::ai_provider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;
    let generation_elapsed = generation_start.elapsed();

    let raw_soap = response.content;
    if raw_soap.is_empty() {
        error!(
            provider = %provider.name(),
            model = %model_name,
            "AI returned an empty SOAP note"
        );
        return Err(AppError::ai_provider(format!(
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

    // Post-process: strip markdown, pull the billing codes out of the note
    // body, and fix paragraph formatting. The stored/returned note carries
    // no code lines — codes go to metadata (`icd_codes`) and render in the
    // frontend's billing-code list, keeping the note clean for reading,
    // copying, and export. (postprocess_soap extracts around the paragraph
    // formatter so the bullet splitter can't orphan code descriptions.)
    let (soap_text, icd_codes) = soap_generator::postprocess_soap(&raw_soap);
    info!(
        icd_codes_extracted = icd_codes.len(),
        "ICD codes extracted from SOAP note"
    );

    // ── Training-corpus capture ─────────────────────────────────────────
    // The two helpers below are the entire training-capture surface of the
    // SOAP path: gated on AppConfig.capture_for_training (default false),
    // best-effort by construction — failures log at warn and NEVER break
    // the SOAP workflow.

    // Validated by load_recording_and_settings; .ok() is purely defensive —
    // a malformed ID yields None and training capture is skipped.
    let recording_uuid = Uuid::parse_str(recording_id).ok();

    let capture_generation_id = capture_training_generation(
        &state.db,
        recording_uuid,
        config.capture_for_training,
        template,
        transcript,
        context,
        patient_context,
        provider.name(),
        &model_name,
        &soap_text,
    );

    // Save context to recording metadata for future reference.
    if recording.metadata.is_null() {
        recording.metadata = serde_json::json!({});
    }
    if let Some(obj) = recording.metadata.as_object_mut() {
        // Always written (even when empty) so the frontend can tell a
        // new-format recording (codes in metadata, clean note) from a
        // legacy one (codes inline in the note, mined as fallback).
        obj.insert(
            "icd_codes".to_string(),
            serde_json::to_value(&icd_codes).unwrap_or_else(|_| serde_json::json!([])),
        );
        if let Some(ctx) = context
            && !ctx.is_empty()
        {
            obj.insert(
                "context".to_string(),
                serde_json::Value::String(ctx.to_string()),
            );
        }
        if let Some(pc) = patient_context
            && !patient_context_is_empty(pc)
        {
            obj.insert(
                "patient_context".to_string(),
                serde_json::to_value(pc).unwrap_or(serde_json::Value::Null),
            );
        }
    }

    medical_core::types::recording::record_completion_stat(
        &mut recording.metadata,
        "soap",
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    );

    // Persist to DB (on blocking thread)
    recording.soap_note = Some(soap_text.clone());
    persist_recording(&state.db, recording).await?;

    finalize_training_generation(&state.db, capture_generation_id, recording_uuid, &soap_text);

    Ok(soap_text)
}

/// Record the draft generation into the `generations` table (training
/// corpus, capture step). Returns the new row's ID when capture actually
/// inserted one — `None` otherwise (capture disabled, recording ID
/// unparseable, DB unavailable, or insert failure). Never errors.
#[allow(clippy::too_many_arguments)]
fn capture_training_generation(
    db: &medical_db::Database,
    recording_uuid: Option<Uuid>,
    capture_enabled: bool,
    template: Option<&str>,
    transcript: &str,
    context: Option<&str>,
    patient_context: Option<&PatientContext>,
    provider_name: &str,
    model_name: &str,
    soap_text: &str,
) -> Option<Uuid> {
    let rec_uuid = recording_uuid?;
    if !capture_enabled {
        return None;
    }
    let conn = match db.conn() {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(error = %e, "training-corpus capture: could not open DB connection; continuing");
            return None;
        }
    };
    let context_blob =
        serde_json::json!({ "context": context, "patient_context": patient_context });
    let context_json = context_blob.to_string();
    let insert = medical_db::generations::GenerationInsert {
        recording_id: rec_uuid,
        output_type: "soap",
        ai_provider: provider_name,
        ai_model: model_name,
        prompt_template_name: template,
        input_transcript: transcript,
        input_context_json: Some(context_json.as_str()),
        draft_text: soap_text,
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
}

/// Mirror the saved text into the generations row's `final_text` and kick
/// off the edit-distance task (training corpus, finalize step). Only runs
/// when capture actually inserted a row above — gating on
/// `capture_generation_id` avoids the GenerationsRepo round trip on every
/// SOAP generation for users who haven't opted into capture. Never errors.
fn finalize_training_generation(
    db: &Arc<medical_db::Database>,
    capture_generation_id: Option<Uuid>,
    recording_uuid: Option<Uuid>,
    soap_text: &str,
) {
    if capture_generation_id.is_none() {
        return;
    }
    // Invariant: capture_generation_id is Some only when recording_uuid was
    // Some (capture_training_generation returns None otherwise). Distrust
    // the invariant instead of panicking on it — this helper is best-effort
    // ("Never errors"), and release builds abort the whole app on panic.
    let Some(rec_uuid) = recording_uuid else {
        tracing::warn!(
            "training-corpus finalize: capture id present but recording id missing; skipping"
        );
        return;
    };
    let conn = match db.conn() {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(error = %e, "training-corpus finalize: could not open DB connection; continuing");
            return;
        }
    };
    match medical_db::generations::GenerationsRepo::update_final_text(
        &conn, rec_uuid, "soap", soap_text,
    ) {
        Ok(Some(g)) => {
            tracing::debug!(generation_id = %g.id, "updated final_text on generations row");
            spawn_edit_distance_task(
                Arc::clone(db),
                g.id,
                g.draft_text.clone(),
                soap_text.to_string(),
            );
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(error = %e, "training-corpus finalize failed; continuing");
        }
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

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::build_test_state_with_recording;
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_soap_returns_endpoint_offline_when_ai_unreachable() {
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
        let result = generate_soap_inner(
            &state,
            None, // app — no AppHandle in tests
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

#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
    use super::*;
    use medical_core::types::recording::GenerationStat;
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_soap_records_generation_stats_in_metadata() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        // Loopback → preflight probe is skipped; the mock serves completions.
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = std::sync::Arc::new(MockCompletionProvider::new(
            "ollama",
            "S: Headache for 3 days.\nA: Tension headache.\nP: Rest, follow up in 2 weeks.",
            200,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let soap = generate_soap_inner(&state, None, &recording_id, None, None, None)
            .await
            .expect("generation with mock provider succeeds");
        assert!(!soap.is_empty());

        let uuid = Uuid::parse_str(&recording_id).expect("valid uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");

        let stat: GenerationStat =
            serde_json::from_value(rec.metadata["generation_stats"]["soap"].clone())
                .expect("soap stat recorded");
        assert_eq!(stat.provider, "ollama");
        assert_eq!(stat.model, "llama3");
        assert_eq!(stat.prompt_tokens, 128);
        assert_eq!(stat.completion_tokens, 200);
        assert!(stat.tokens_per_second.is_finite());
        assert!(stat.tokens_per_second > 0.0);

        assert_eq!(
            medical_core::types::recording::latest_tokens_per_second(&rec.metadata),
            Some(stat.tokens_per_second)
        );
    }

    /// The stored + returned SOAP note must be free of ICD code lines; the
    /// codes move to `metadata.icd_codes` for the billing-code list.
    #[tokio::test]
    async fn generate_soap_strips_icd_lines_into_metadata() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = std::sync::Arc::new(MockCompletionProvider::new(
            "ollama",
            "ICD-9 Code: 847.2 — Sprain of lumbar\nICD-9 Code: 724.5 — Lumbago\nICD-9 Code: 719.43 - Pain in ankle\n\nSubjective:\n- Chief complaint: back pain\n\nAssessment:\n- Lumbar strain",
            200,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports back pain after lifting boxes.",
            provider,
        )
        .await;

        let soap = generate_soap_inner(&state, None, &recording_id, None, None, None)
            .await
            .expect("generation with mock provider succeeds");

        // Returned text: no code lines, clinical content intact, and the
        // hyphen-separated description is not stranded as an orphan bullet.
        assert!(
            !soap.contains("ICD-9"),
            "returned note must be code-free: {soap}"
        );
        assert!(
            !soap.contains("- Pain in ankle"),
            "no orphaned description bullet: {soap}"
        );
        assert!(soap.contains("Subjective:"));
        assert!(soap.contains("Lumbar strain"));

        let uuid = Uuid::parse_str(&recording_id).expect("valid uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");

        // Persisted note: also code-free.
        let stored = rec.soap_note.as_deref().expect("soap_note persisted");
        assert!(
            !stored.contains("ICD-9"),
            "stored note must be code-free: {stored}"
        );

        // Metadata: structured codes with their model-written titles — the
        // hyphen-separated line keeps its description too (bullet-split
        // regression).
        let codes = rec.metadata["icd_codes"]
            .as_array()
            .expect("icd_codes array in metadata");
        assert_eq!(codes.len(), 3);
        assert_eq!(codes[0]["code"], "847.2");
        assert_eq!(codes[0]["description"], "Sprain of lumbar");
        assert_eq!(codes[0]["kind"], "icd9");
        assert_eq!(codes[1]["code"], "724.5");
        assert_eq!(codes[2]["code"], "719.43");
        assert_eq!(codes[2]["description"], "Pain in ankle");
    }

    /// A note with no code lines must round-trip untouched, with an empty
    /// `icd_codes` array still written (new-format marker).
    #[tokio::test]
    async fn generate_soap_without_codes_writes_empty_metadata_array() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = std::sync::Arc::new(MockCompletionProvider::new(
            "ollama",
            "Subjective:\n- Headache for 3 days.\n\nPlan:\n- Rest",
            200,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let soap = generate_soap_inner(&state, None, &recording_id, None, None, None)
            .await
            .expect("generation with mock provider succeeds");
        assert!(soap.contains("Headache for 3 days."));

        let uuid = Uuid::parse_str(&recording_id).expect("valid uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");
        assert_eq!(
            rec.metadata["icd_codes"].as_array().map(Vec::len),
            Some(0),
            "empty icd_codes array written as new-format marker"
        );
    }
    /// The finalize helper is documented "Never errors" and must not panic
    /// (release builds abort the whole app on panic) even if the
    /// capture-id/recording-id invariant is ever broken by a caller change.
    /// Previously this path hit `.expect(...)` — tech-debt review 2026-08-25.
    #[test]
    fn finalize_skips_gracefully_when_invariant_broken() {
        let db = medical_db::Database::open_in_memory().expect("db");
        // Capture id present but recording id missing — returns after the
        // warn, without touching the DB.
        finalize_training_generation(&Arc::new(db), Some(Uuid::new_v4()), None, "S: ok");
    }
}
