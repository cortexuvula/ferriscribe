//! `generate_soap` Tauri command — turns a recording's transcript into a SOAP note.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::PatientContext;
use medical_processing::soap_generator::{self, SoapPromptConfig};
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

use crate::state::AppState;

use super::helpers::{
    acquire_generation_lock, build_completion_request, ensure_nonempty_output,
    ensure_prompt_within_cap, load_recording_and_settings, patient_context_is_empty,
    require_transcript, resolve_provider, resolve_soap_template, run_generation_command,
    stream_with_events, validate_patient_context,
};

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
    // One generation per recording at a time — the pipeline and the
    // Generate/Record tabs go through different UI stores, so the backend
    // enforces it. RAII guard releases on every exit path.
    let _generation_lock = acquire_generation_lock(&state, &recording_id)?;

    // Reject a malformed structured context up front (the freeform-context
    // size cap runs inside run_generation_command, before "started").
    if let Some(ref pc) = patient_context {
        validate_patient_context(pc)?;
    }

    let inner = generate_soap_inner(
        &state,
        Some(&app),
        &recording_id,
        template.as_deref(),
        context.as_deref(),
        patient_context.as_ref(),
    );
    run_generation_command(&app, &recording_id, "soap", context.as_deref(), inner).await
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
    let (recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // A custom system prompt replaces the ~12K-char default wholesale;
    // bound it to the app-wide user-text budget so a pathological paste
    // can't silently consume the model's whole context window. Mirrors the
    // save-time check in `save_settings` (this one also covers configs
    // that arrived via sync).
    ensure_prompt_within_cap(settings.custom_soap_prompt.as_deref(), "SOAP")?;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::GenerateSoap,
        &config,
    )
    .await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let transcript = require_transcript(&recording)?;

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

    let (response, generation_elapsed) =
        stream_with_events(&provider, app, "soap", recording_id, request).await?;

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
    // Post-processing can empty a completion the raw check above let through:
    // whitespace-only output (trimmed by clean_text), a note consisting solely
    // of ICD code lines (extracted to metadata), or one wrapped entirely in a
    // fenced code block (clean_text removes fences with their content).
    // Reject BEFORE persisting — otherwise an empty note is stored and
    // reported as success. Same trimmed-empty rule the other generators
    // enforce via this shared helper.
    ensure_nonempty_output(&soap_text, "SOAP note")?;
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

    // Build the metadata PATCH (applied to the row's CURRENT metadata at
    // persist time — the snapshot may be minutes stale, and a wholesale
    // metadata write would drop concurrent key writes). `icd_codes` is
    // always written (even when empty) so the frontend can tell a
    // new-format recording (codes in metadata, clean note) from a legacy
    // one (codes inline in the note, mined as fallback). `context` and
    // `patient_context` are likewise written unconditionally — null when
    // absent — so the metadata always mirrors the inputs of the CURRENT
    // note. Writing them only when non-empty left a prior generation's
    // context attached to a note that was regenerated without it (the
    // fields repopulate from metadata on the next recording switch). All
    // readers treat null like an absent key (frontend
    // `contextFromMetadata` type-checks strings; the sync wire builder
    // reads via `as_str()`).
    let mut metadata_patch: Vec<(String, serde_json::Value)> = vec![(
        "icd_codes".to_string(),
        serde_json::to_value(&icd_codes).unwrap_or_else(|_| serde_json::json!([])),
    )];
    metadata_patch.push((
        "context".to_string(),
        context
            .filter(|c| !c.is_empty())
            .map(|c| serde_json::Value::String(c.to_string()))
            .unwrap_or(serde_json::Value::Null),
    ));
    metadata_patch.push((
        "patient_context".to_string(),
        patient_context
            .filter(|pc| !patient_context_is_empty(pc))
            .and_then(|pc| serde_json::to_value(pc).ok())
            .unwrap_or(serde_json::Value::Null),
    ));

    // The stat merges under generation_stats.soap — record it on a scratch
    // object and ship just that sub-object as the patch entry (the persist
    // merges one level, preserving sibling doc-type stats).
    let mut stats_scratch = serde_json::json!({});
    medical_core::types::recording::record_completion_stat(
        &mut stats_scratch,
        "soap",
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    );
    if let Some(stats) = stats_scratch.get("generation_stats") {
        metadata_patch.push(("generation_stats".to_string(), stats.clone()));
    }

    // Targeted producer persist (blocking thread): the recording snapshot
    // is stale by however long the LLM ran — a whole-row update would
    // revert any column the editor saved meanwhile. Only soap_note plus
    // the metadata patch are written; updated_at still bumps.
    {
        let db = Arc::clone(&state.db);
        let persist_id = recording.id;
        let persist_soap = soap_text.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let conn = db.conn()?;
            medical_db::recordings::RecordingsRepo::persist_producer_update(
                &conn,
                &persist_id,
                &medical_db::recordings::ProducerPersist {
                    soap_note: Some(persist_soap),
                    metadata_patch,
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .map_err(crate::commands::join_err)??;
    }

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
    use super::super::test_helpers::{assert_endpoint_offline, build_test_state_with_recording};
    use super::*;
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
        assert_endpoint_offline(result, "Ollama", start);
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

/// Regression tests (SOAP pipeline review 2026-09-04): a completion that
/// post-processing empties must be rejected before anything is persisted,
/// and a regeneration without context must clear the stale metadata keys.
#[cfg(test)]
mod postprocess_rejection_tests {
    use super::super::MAX_CONTEXT_CHARS;
    use super::super::test_helpers::{MockCompletionProvider, build_test_state_with_provider};
    use super::*;
    use medical_core::types::settings::AppConfig;

    fn base_config() -> AppConfig {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        // Loopback → preflight probe is skipped; the mock serves completions.
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();
        config
    }

    fn note_provider() -> std::sync::Arc<MockCompletionProvider> {
        std::sync::Arc::new(MockCompletionProvider::new(
            "ollama",
            "Subjective:\n- Chief complaint: back pain\n\nPlan:\n- Rest",
            200,
        ))
    }

    async fn loaded_recording(
        state: &AppState,
        recording_id: &str,
    ) -> medical_core::types::recording::Recording {
        let uuid = Uuid::parse_str(recording_id).expect("valid uuid");
        let conn = state.db.conn().expect("conn");
        medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording row")
    }

    /// A completion consisting solely of ICD code lines passes the raw
    /// emptiness check (the model DID return content) but post-processing
    /// extracts every line away. Previously the empty note was persisted,
    /// a "completed" progress event emitted, and the success toast shown.
    #[tokio::test]
    async fn generate_soap_rejects_codes_only_completion_without_persisting() {
        let provider = std::sync::Arc::new(MockCompletionProvider::new(
            "ollama",
            "ICD-9 Code: 847.2 — Sprain of lumbar\nICD-9 Code: 724.5 — Lumbago",
            200,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            base_config(),
            "Patient reports back pain after lifting boxes.",
            provider,
        )
        .await;

        let result = generate_soap_inner(&state, None, &recording_id, None, None, None).await;
        let err = result.expect_err("codes-only completion must be rejected");
        assert!(
            err.to_string().contains("empty"),
            "expected empty-note error, got: {err}"
        );

        let rec = loaded_recording(&state, &recording_id).await;
        assert!(
            rec.soap_note.is_none(),
            "no empty note persisted: {:?}",
            rec.soap_note
        );
        assert!(
            rec.metadata.get("icd_codes").is_none(),
            "no metadata patch applied on rejection: {}",
            rec.metadata
        );
    }

    /// Whitespace-only output also becomes an empty note after clean_text
    /// trims it — same "rejected like empty everywhere" rule the other
    /// generators already enforce.
    #[tokio::test]
    async fn generate_soap_rejects_whitespace_only_completion() {
        let provider = std::sync::Arc::new(MockCompletionProvider::new("ollama", "   \n\t  ", 200));
        let (state, recording_id) = build_test_state_with_provider(
            base_config(),
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let result = generate_soap_inner(&state, None, &recording_id, None, None, None).await;
        assert!(
            result.is_err(),
            "whitespace-only completion must be rejected"
        );

        let rec = loaded_recording(&state, &recording_id).await;
        assert!(rec.soap_note.is_none(), "no empty note persisted");
    }

    /// Regenerating without context/patient-context must CLEAR the previous
    /// generation's values from metadata — writing them only when non-empty
    /// left a stale prior context attached to the new note, and the fields
    /// repopulate from metadata on the next recording switch.
    #[tokio::test]
    async fn generate_soap_clears_stale_context_metadata_when_regenerated_without() {
        let (state, recording_id) = build_test_state_with_provider(
            base_config(),
            "Patient reports back pain after lifting boxes.",
            note_provider(),
        )
        .await;

        let pc = PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: vec!["Lisinopril 10mg PO daily".into()],
            allergies: vec![],
            conditions: vec!["Hypertension".into()],
        };

        // First generation WITH context — metadata records both inputs.
        generate_soap_inner(
            &state,
            None,
            &recording_id,
            None,
            Some("prior visit notes"),
            Some(&pc),
        )
        .await
        .expect("first generation succeeds");
        let rec = loaded_recording(&state, &recording_id).await;
        assert_eq!(
            rec.metadata["context"],
            serde_json::json!("prior visit notes")
        );
        assert_eq!(
            rec.metadata["patient_context"]["medications"][0],
            serde_json::json!("Lisinopril 10mg PO daily")
        );

        // Regeneration WITHOUT them — the stale values must be cleared to
        // null, not silently retained.
        generate_soap_inner(&state, None, &recording_id, None, None, None)
            .await
            .expect("second generation succeeds");
        let rec = loaded_recording(&state, &recording_id).await;
        assert!(
            rec.metadata
                .get("context")
                .is_none_or(serde_json::Value::is_null),
            "stale context must be cleared: {}",
            rec.metadata
        );
        assert!(
            rec.metadata
                .get("patient_context")
                .is_none_or(serde_json::Value::is_null),
            "stale patient_context must be cleared: {}",
            rec.metadata
        );
        // The note itself survives both runs.
        assert!(rec.soap_note.is_some_and(|n| n.contains("back pain")));
    }

    /// A soft-deleted recording must fail FAST — previously the generation
    /// loaded the trashed row, spent the whole LLM call, and only then
    /// failed the persist (which filters deleted_at) with a bare NotFound.
    #[tokio::test]
    async fn generate_soap_fails_fast_for_soft_deleted_recording() {
        let (state, recording_id) = build_test_state_with_provider(
            base_config(),
            "Patient reports back pain after lifting boxes.",
            note_provider(),
        )
        .await;

        let uuid = Uuid::parse_str(&recording_id).expect("valid uuid");
        {
            let conn = state.db.conn().expect("conn");
            medical_db::recordings::RecordingsRepo::soft_delete(&conn, &uuid).expect("soft delete");
        }

        let result = generate_soap_inner(&state, None, &recording_id, None, None, None).await;
        let err = result.expect_err("generation on trashed recording must fail");
        assert!(
            err.to_string().contains("deleted"),
            "expected 'is deleted' error, got: {err}"
        );

        let rec = loaded_recording(&state, &recording_id).await;
        assert!(rec.soap_note.is_none(), "no LLM output persisted");
    }

    /// An oversized custom SOAP prompt (synced in or saved before the cap
    /// existed) is rejected at generation time before any provider call.
    #[tokio::test]
    async fn generate_soap_rejects_oversized_custom_prompt() {
        let mut config = base_config();
        config.custom_soap_prompt = Some("p".repeat(MAX_CONTEXT_CHARS + 1));

        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports back pain after lifting boxes.",
            note_provider(),
        )
        .await;

        let result = generate_soap_inner(&state, None, &recording_id, None, None, None).await;
        let err = result.expect_err("oversized custom prompt must be rejected");
        assert!(
            matches!(err, AppError::InvalidInput(_)),
            "expected InvalidInput, got: {err}"
        );
        assert!(err.to_string().contains("Custom SOAP prompt too large"));

        let rec = loaded_recording(&state, &recording_id).await;
        assert!(rec.soap_note.is_none(), "no LLM output persisted");
    }
}
