//! Shared helpers for the four generation commands.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::recording::{Recording, record_completion_stat};
use medical_core::types::settings::{AppConfig, IcdVersion, SoapTemplate};
use medical_core::types::{CompletionRequest, Message, MessageContent, PatientContext, Role};
use medical_db::recordings::RecordingsRepo;
use tauri::Emitter;
use uuid::Uuid;

use crate::state::AppState;

use super::{
    GenerationProgress, MAX_CONTEXT_CHARS, MAX_SOAP_NOTE_CHARS, MAX_TRANSCRIPT_CHARS,
    PATIENT_CTX_MAX_ITEM_CHARS, PATIENT_CTX_MAX_ITEMS_PER_LIST, format_progress_error,
};

/// Loaded settings needed for generation.
pub(super) struct GenerationSettings {
    pub model: String,
    pub temperature: f32,
    pub icd_version: IcdVersion,
    pub ai_provider: String,
    pub custom_soap_prompt: Option<String>,
    pub custom_referral_prompt: Option<String>,
    pub custom_letter_prompt: Option<String>,
    pub custom_synopsis_prompt: Option<String>,
    pub custom_peer_discussion_prompt: Option<String>,
}

/// Load a recording, generation settings, and the full `AppConfig` from the DB
/// on a blocking thread.
///
/// All rusqlite work is offloaded via `spawn_blocking` so we never block the
/// Tokio async runtime.
///
/// Returns `(Recording, GenerationSettings, AppConfig)`. The `AppConfig` is
/// available to callers that need to pass it directly to
/// `preflight_for_command`, avoiding a second `spawn_blocking` config load.
pub(super) async fn load_recording_and_settings(
    db: &Arc<medical_db::Database>,
    recording_id: &str,
) -> AppResult<(Recording, GenerationSettings, AppConfig)> {
    let uuid = Uuid::parse_str(recording_id)
        .map_err(|e| AppError::InvalidInput(format!("Invalid recording ID: {e}")))?;
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;

        // Active-only lookup: generation holds this snapshot across a
        // minutes-long LLM call, and the persist at the end filters
        // `deleted_at IS NULL` — loading a trashed row here would spend the
        // whole completion and then fail with a bare NotFound
        // (get_by_id_active names the deletion instead).
        let recording = RecordingsRepo::get_by_id_active(&conn, &uuid)?;

        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)?;
        config.migrate();

        let settings = GenerationSettings {
            model: config.ai_model.clone(),
            temperature: config.temperature,
            icd_version: config.icd_version.clone(),
            ai_provider: config.ai_provider.clone(),
            custom_soap_prompt: config.custom_soap_prompt.clone(),
            custom_referral_prompt: config.custom_referral_prompt.clone(),
            custom_letter_prompt: config.custom_letter_prompt.clone(),
            custom_synopsis_prompt: config.custom_synopsis_prompt.clone(),
            custom_peer_discussion_prompt: config.custom_peer_discussion_prompt.clone(),
        };

        Ok::<_, AppError>((recording, settings, config))
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Load the full `AppConfig` from the DB on a blocking worker, without a
/// recording. Used by commands that don't operate on a recording (e.g. the
/// standalone Letter Writer, which generates from OCR'd text).
pub(super) async fn load_config(db: &Arc<medical_db::Database>) -> AppResult<AppConfig> {
    crate::commands::load_app_config(db, "generation").await
}

/// Resolve the AI provider from the registry using the settings provider name.
///
/// `pub` so it can be re-exported from `generation::mod` for `commands::ocr`;
/// the `helpers` module itself is private to `generation`, so this does not
/// widen the public surface beyond the crate.
pub async fn resolve_provider(
    state: &AppState,
    provider_name: &str,
) -> AppResult<Arc<dyn AiProvider>> {
    let registry = state.ai_providers.lock().await;
    registry
        .get_arc(provider_name)
        .or_else(|| registry.get_active_arc())
        .ok_or_else(|| {
            AppError::ai_provider(
                "No AI provider configured. Check LM Studio / Ollama / oMLX settings.".to_string(),
            )
        })
}

/// Persist a recording update on a blocking thread.
/// Column-scoped producer persist (see
/// [`RecordingsRepo::persist_producer_update`]) for the document
/// generators. These hold their recording snapshot across a long LLM
/// call — a whole-row update would silently revert any column another
/// writer changed in the window (the editor's autosave, a concurrent
/// generator). Runs the DB write on a blocking worker.
pub(super) async fn persist_producer_patch(
    state: &AppState,
    recording_id: Uuid,
    update: medical_db::recordings::ProducerPersist,
) -> AppResult<()> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn()?;
        RecordingsRepo::persist_producer_update(&conn, &recording_id, &update)
            .map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
}

/// Build the metadata patch carrying just this doc-type's FRESH
/// generation stats — extracted from the in-memory recording where
/// `record_completion_stat` wrote them moments ago. The persist-time
/// one-level merge preserves sibling doc-type stats in the DB row.
pub(super) fn fresh_stats_patch(
    recording: &Recording,
    stats_key: &str,
) -> Vec<(String, serde_json::Value)> {
    recording
        .metadata
        .get("generation_stats")
        .and_then(|stats| stats.get(stats_key))
        .map(|stat| {
            vec![(
                "generation_stats".to_string(),
                serde_json::json!({ stats_key: stat.clone() }),
            )]
        })
        .unwrap_or_default()
}

/// Runs a generation command with the standard progress-event lifecycle:
/// validates context size, emits "started", calls the inner function,
/// then emits "completed" or "failed".
///
/// `doc_type` is the string literal sent to the frontend in progress events
/// (e.g. `"letter"`, `"referral"`). `context` is the user-supplied freeform
/// context, validated against [`MAX_CONTEXT_CHARS`] before any work begins.
pub(super) async fn run_generation_command(
    app: &tauri::AppHandle,
    recording_id: &str,
    doc_type: &str,
    context: Option<&str>,
    inner: impl std::future::Future<Output = AppResult<String>>,
) -> AppResult<String> {
    // Validate context size before emitting started (fail fast, consistent with SOAP).
    if let Some(ctx) = context
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
            doc_type: doc_type.into(),
            status: "started".into(),
            recording_id: recording_id.to_string(),
            progress: None,
        },
    );

    let result = inner.await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: doc_type.into(),
                    status: "completed".into(),
                    recording_id: recording_id.to_string(),
                    progress: None,
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: doc_type.into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.to_string(),
                    progress: None,
                },
            );
        }
    }

    result
}

/// RAII guard: removes the recording's key from the in-flight set on drop
/// (success, error, or panic unwind) so an aborted generation never
/// permanently blocks future ones for that recording.
#[derive(Debug)]
pub(super) struct GenerationLockGuard {
    locks: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    key: String,
}

impl Drop for GenerationLockGuard {
    fn drop(&mut self) {
        match self.locks.lock() {
            Ok(mut guard) => {
                guard.remove(&self.key);
            }
            Err(poisoned) => {
                tracing::error!(
                    key = %self.key,
                    "generation lock set poisoned; entry leaked. \
                     Subsequent generations for this recording may be rejected."
                );
                // Best-effort cleanup using the poisoned inner.
                poisoned.into_inner().remove(&self.key);
            }
        }
    }
}

/// Reject a second concurrent generation for the same recording.
///
/// The UI's `generation` store serializes same-tab clicks, but the
/// background pipeline and the Generate/Record tabs go through different
/// stores — without this, the backend could run two LLM generations on one
/// row at once (interleaved `generation-progress` events, double provider
/// cost, last-writer-wins persist, and with training capture on,
/// `update_final_text`'s latest-row semantics could cross-finalize the
/// other run's draft).
pub(super) fn acquire_generation_lock(
    state: &AppState,
    recording_id: &str,
) -> AppResult<GenerationLockGuard> {
    let mut guard = state
        .generation_locks
        .lock()
        .map_err(|e| AppError::MutexPoisoned(format!("generation_locks: {e}")))?;
    if guard.contains(recording_id) {
        return Err(AppError::Other(
            "a generation is already running for this recording".to_string(),
        ));
    }
    guard.insert(recording_id.to_string());
    Ok(GenerationLockGuard {
        locks: Arc::clone(&state.generation_locks),
        key: recording_id.to_string(),
    })
}

/// Shared inner logic for document types generated from a SOAP note (referral,
/// letter, synopsis). Handles: preflight, resolve provider, validate SOAP note,
/// build completion request, stream the completion from the provider (emitting
/// live `generation-progress` stats through `app` when present), strip
/// markdown, check empty, persist.
///
/// The recording, settings, and config must already be loaded by the caller
/// (via [`load_recording_and_settings`]); this keeps the loading I/O to a
/// single DB round-trip and lets callers (e.g. `generate_letter`) do extra
/// DB work between the load and the generation.
///
/// The unique per-call pieces are passed in:
/// - `command_kind` / `config`: forwarded to `preflight_for_command`.
/// - `doc_type_label`: human-readable name used in the "empty response" error
///   and the success debug log (e.g. `"letter"`, `"referral letter"`).
/// - `stats_key`: canonical key under metadata.generation_stats (e.g. "referral").
/// - `build_prompt`: closure `(soap_note, &settings) -> (system_prompt,
///   user_prompt)`.
/// - `set_field`: closure `(&mut Recording, String)` that assigns the
///   generated text to the right recording field.
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate_from_soap<F, S>(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording: &mut Recording,
    settings: &GenerationSettings,
    config: &AppConfig,
    command_kind: medical_core::preflight::CommandKind,
    doc_type_label: &str,
    stats_key: &'static str,
    build_prompt: F,
    set_field: S,
) -> AppResult<String>
where
    F: FnOnce(&str, &GenerationSettings) -> (String, String),
    S: FnOnce(&mut Recording, String),
{
    use medical_processing::document_generator;

    // Pre-flight: probe the remote AI endpoint before doing any work.
    // Skipped for loopback hosts; returns EndpointOffline on failure
    // without ever invoking the provider.
    medical_core::preflight::preflight_for_command(command_kind, config).await?;

    let provider = resolve_provider(state, &settings.ai_provider).await?;

    let soap_note = require_soap_note(recording)?;

    let (system_prompt, user_prompt) = build_prompt(soap_note, settings);

    tracing::debug!(
        doc_type = doc_type_label,
        provider = %provider.name(),
        recording_id = %recording.id,
        "generating document from SOAP note"
    );

    let request = build_completion_request(
        system_prompt,
        user_prompt,
        settings.model.clone(),
        settings.temperature,
        None,
    );

    let recording_id_str = recording.id.to_string();
    let (response, generation_elapsed) =
        stream_with_events(&provider, app, stats_key, &recording_id_str, request).await?;

    let text = document_generator::strip_markdown(&response.content);
    ensure_nonempty_output(&text, doc_type_label)?;

    set_field(recording, text.clone());

    record_completion_stat(
        &mut recording.metadata,
        stats_key,
        provider.name(),
        &settings.model,
        &response.usage,
        generation_elapsed,
    );

    Ok(text)
}

/// Borrow the recording's transcript, rejecting recordings without one and
/// transcripts over the size cap. Shared by the transcript-based
/// generators (SOAP, peer discussion).
pub(super) fn require_transcript(recording: &Recording) -> AppResult<&str> {
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
    Ok(transcript)
}

/// Borrow the recording's SOAP note, rejecting recordings without one and
/// notes over the size cap. Shared by the SOAP-derived generators
/// (referral, letter, synopsis).
pub(super) fn require_soap_note(recording: &Recording) -> AppResult<&str> {
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
    Ok(soap_note)
}

/// Drive the streamed completion with live `generation-progress` events,
/// preserving `EndpointOffline` verbatim (the frontend dialog keys on it),
/// and time the call for the generation stat. Shared by every streaming
/// generator; the command-specific pieces (prompt building, post-processing,
/// persistence) stay at the call sites.
pub(super) async fn stream_with_events(
    provider: &Arc<dyn AiProvider>,
    app: Option<&tauri::AppHandle>,
    doc_type: &str,
    recording_id: &str,
    request: CompletionRequest,
) -> AppResult<(medical_core::types::CompletionResponse, std::time::Duration)> {
    let generation_start = std::time::Instant::now();
    let response = super::stream::stream_to_completion(
        provider,
        |stats| {
            if let Some(app) = app {
                let _ = app.emit(
                    "generation-progress",
                    GenerationProgress {
                        doc_type: doc_type.to_string(),
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
    Ok((response, generation_start.elapsed()))
}

/// Reject an empty (or whitespace-only) completion with the shared wording.
pub(super) fn ensure_nonempty_output(text: &str, doc_type_label: &str) -> AppResult<()> {
    if text.trim().is_empty() {
        return Err(AppError::ai_provider(format!(
            "AI returned an empty {doc_type_label}."
        )));
    }
    Ok(())
}

/// Parse a template string into the `SoapTemplate` enum.
///
/// Returns `None` for unrecognized strings — [`resolve_soap_template`]
/// then falls back to the user's stored preference rather than a hard-coded
/// default. Accepts the enum's snake_case wire forms (plus common
/// punctuation variants of follow-up).
pub(super) fn parse_soap_template(s: &str) -> Option<SoapTemplate> {
    match s.to_lowercase().as_str() {
        "follow_up" | "followup" | "follow-up" => Some(SoapTemplate::FollowUp),
        "new_patient" | "newpatient" => Some(SoapTemplate::NewPatient),
        "telehealth" => Some(SoapTemplate::Telehealth),
        "emergency" => Some(SoapTemplate::Emergency),
        "pediatric" => Some(SoapTemplate::Pediatric),
        "geriatric" => Some(SoapTemplate::Geriatric),
        _ => None,
    }
}

/// Resolve the effective SOAP template: an explicit request wins, otherwise
/// the user's stored preference (`AppConfig.soap_template`).
///
/// This lives here — not in the pipeline command — so that EVERY caller
/// which passes no template (Generate tab, Regenerate, the pipeline) honors
/// the stored setting. Previously only the pipeline did its own settings
/// lookup, and a silent error there (or a direct `generate_soap` call)
/// fell back to FollowUp regardless of the user's configured template.
///
/// An explicit but unparseable string ALSO falls back to the stored
/// preference (2026-09-04 review): silently discarding it in favor of
/// FollowUp ignored the user's configured default.
pub(super) fn resolve_soap_template(template: Option<&str>, config: &AppConfig) -> SoapTemplate {
    match template {
        Some(t) => parse_soap_template(t).unwrap_or_else(|| config.soap_template.clone()),
        None => config.soap_template.clone(),
    }
}

/// Build a single-turn `CompletionRequest` from system and user prompts.
pub(super) fn build_completion_request(
    system_prompt: String,
    user_prompt: String,
    model: String,
    temperature: f32,
    max_tokens: Option<u32>,
) -> CompletionRequest {
    CompletionRequest {
        model,
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(user_prompt),
            tool_calls: vec![],
        }],
        temperature: Some(temperature),
        max_tokens,
        system_prompt: Some(system_prompt),
        // Thinking control is applied provider-side (Ollama forces
        // reasoning_effort; LM Studio injects a think-block prefill), so the
        // generation layer always leaves the request neutral here.
        reasoning_effort: None,
    }
}

/// Validate a structured `PatientContext` against the per-list and per-item
/// caps. The caps protect against pathological input (e.g. a 50K paste into
/// a single med field) and total-payload bloat.
///
/// Total character budget reuses `MAX_CONTEXT_CHARS` for symmetry with
/// the freeform-context cap, which already exists for the same purpose.
pub(crate) fn validate_patient_context(pc: &PatientContext) -> AppResult<()> {
    let lists: [(&str, &[String]); 3] = [
        ("medications", &pc.medications),
        ("allergies", &pc.allergies),
        ("conditions", &pc.conditions),
    ];

    let mut total: usize = 0;
    for (label, items) in lists {
        if items.len() > PATIENT_CTX_MAX_ITEMS_PER_LIST {
            return Err(AppError::InvalidInput(format!(
                "Too many {label} entries: {} (limit is {})",
                items.len(),
                PATIENT_CTX_MAX_ITEMS_PER_LIST
            )));
        }
        for item in items {
            if item.len() > PATIENT_CTX_MAX_ITEM_CHARS {
                return Err(AppError::InvalidInput(format!(
                    "Patient context entry too long in {label}: {} chars (limit is {})",
                    item.len(),
                    PATIENT_CTX_MAX_ITEM_CHARS
                )));
            }
            total += item.len();
        }
    }

    if total > MAX_CONTEXT_CHARS {
        return Err(AppError::InvalidInput(format!(
            "Patient context too large: {total} chars (limit is {MAX_CONTEXT_CHARS})"
        )));
    }

    Ok(())
}

/// True iff every surfaced list (`medications`, `allergies`, `conditions`)
/// is empty. Such a payload contributes nothing to the prompt and must not
/// be persisted, so the recording metadata stays clean.
pub(super) fn patient_context_is_empty(pc: &PatientContext) -> bool {
    pc.medications.is_empty() && pc.allergies.is_empty() && pc.conditions.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_soap_template_falls_back_to_stored_preference() {
        // The bug this pins: a caller passing no template must get the
        // user's configured template, not a silent FollowUp default.
        let mut config = AppConfig::default();
        config.soap_template = SoapTemplate::Pediatric;
        assert_eq!(
            resolve_soap_template(None, &config),
            SoapTemplate::Pediatric
        );
    }

    #[test]
    fn resolve_soap_template_explicit_request_wins() {
        let mut config = AppConfig::default();
        config.soap_template = SoapTemplate::Pediatric;
        assert_eq!(
            resolve_soap_template(Some("telehealth"), &config),
            SoapTemplate::Telehealth
        );
    }

    #[test]
    fn resolve_soap_template_unparseable_string_falls_back_to_stored_preference() {
        // 2026-09-04 review: an explicit-but-garbage string previously
        // silently discarded the user's stored preference in favor of the
        // hard-coded FollowUp default.
        let mut config = AppConfig::default();
        config.soap_template = SoapTemplate::Geriatric;
        assert_eq!(
            resolve_soap_template(Some("not-a-template"), &config),
            SoapTemplate::Geriatric
        );
    }

    #[test]
    fn resolve_soap_template_explicit_follow_up_wins_over_stored_preference() {
        let mut config = AppConfig::default();
        config.soap_template = SoapTemplate::Geriatric;
        assert_eq!(
            resolve_soap_template(Some("follow_up"), &config),
            SoapTemplate::FollowUp,
            "an explicit FollowUp must not fall through to the stored preference"
        );
    }

    #[test]
    fn parse_soap_template_accepts_snake_case_wire_forms() {
        // The enum serializes snake_case (serde rename_all) — every wire
        // form must parse now that None means "use stored preference".
        assert_eq!(
            parse_soap_template("follow_up"),
            Some(SoapTemplate::FollowUp)
        );
        assert_eq!(
            parse_soap_template("new_patient"),
            Some(SoapTemplate::NewPatient)
        );
        assert_eq!(
            parse_soap_template("telehealth"),
            Some(SoapTemplate::Telehealth)
        );
        assert_eq!(
            parse_soap_template("emergency"),
            Some(SoapTemplate::Emergency)
        );
        assert_eq!(
            parse_soap_template("pediatric"),
            Some(SoapTemplate::Pediatric)
        );
        assert_eq!(
            parse_soap_template("geriatric"),
            Some(SoapTemplate::Geriatric)
        );
        assert_eq!(
            parse_soap_template("Telehealth"),
            Some(SoapTemplate::Telehealth)
        );
        assert_eq!(parse_soap_template("nonsense"), None);
    }

    #[tokio::test]
    async fn generation_lock_serializes_per_recording_and_releases_on_drop() {
        let (state, recording_id) = super::super::test_helpers::build_test_state_with_recording(
            AppConfig::default(),
            "transcript",
        )
        .await;

        let first = acquire_generation_lock(&state, &recording_id).expect("first acquire");
        let err = acquire_generation_lock(&state, &recording_id).expect_err("second acquire");
        assert!(
            err.to_string().contains("already running"),
            "expected already-running error: {err}"
        );
        // A different recording is unaffected.
        let other = acquire_generation_lock(&state, "other-recording")
            .expect("different recording acquires");
        drop(other);

        drop(first);
        let reacquired = acquire_generation_lock(&state, &recording_id)
            .expect("lock released on drop, re-acquire works");
        drop(reacquired);
    }

    #[test]
    fn resolve_soap_template_default_config_is_follow_up() {
        assert_eq!(
            resolve_soap_template(None, &AppConfig::default()),
            SoapTemplate::FollowUp
        );
    }

    fn pc(meds: &[&str], allergies: &[&str], conditions: &[&str]) -> PatientContext {
        PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: meds.iter().map(|s| (*s).to_string()).collect(),
            allergies: allergies.iter().map(|s| (*s).to_string()).collect(),
            conditions: conditions.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn validate_patient_context_accepts_normal_payload() {
        let ctx = pc(
            &["Lisinopril 10mg daily", "Metformin 500mg BID"],
            &["Penicillin"],
            &["HTN", "T2DM"],
        );
        assert!(validate_patient_context(&ctx).is_ok());
    }

    #[test]
    fn validate_patient_context_accepts_all_empty() {
        let ctx = pc(&[], &[], &[]);
        assert!(validate_patient_context(&ctx).is_ok());
    }

    #[test]
    fn validate_patient_context_rejects_total_too_large() {
        // Each item is exactly at the per-item cap (500 chars) so it does not
        // trip the per-item check; lists are at the per-list cap (50) so the
        // count check also passes. Total = 50*500 + 50*500 + 1*500 = 50_500,
        // which exceeds MAX_CONTEXT_CHARS (50_000) by 500 — so only the
        // total-cap check should fire.
        let big = "x".repeat(500);
        let fifty: Vec<&str> = std::iter::repeat_n(big.as_str(), 50).collect();
        let mut ctx = pc(&fifty, &fifty, &[]);
        ctx.conditions.push(big.clone());
        let err = validate_patient_context(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("too large"),
            "expected 'too large' in error: {msg}"
        );
    }

    #[test]
    fn validate_patient_context_rejects_too_many_items() {
        let many: Vec<String> = (0..51).map(|i| format!("med-{i}")).collect();
        let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let ctx = pc(&many_refs, &[], &[]);
        let err = validate_patient_context(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("too many") || msg.contains("50"),
            "expected too-many error: {msg}"
        );
    }

    #[test]
    fn validate_patient_context_rejects_item_too_long() {
        let long = "y".repeat(501);
        let ctx = pc(&[long.as_str()], &[], &[]);
        let err = validate_patient_context(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("too long") || msg.contains("500"),
            "expected too-long error: {msg}"
        );
    }
}
