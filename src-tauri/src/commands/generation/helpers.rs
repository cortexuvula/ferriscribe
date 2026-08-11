//! Shared helpers for the four generation commands.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::recording::Recording;
use medical_core::types::settings::{AppConfig, IcdVersion, SoapTemplate};
use medical_core::types::{CompletionRequest, Message, MessageContent, PatientContext, Role};
use medical_db::recordings::RecordingsRepo;
use tauri::Emitter;
use uuid::Uuid;

use crate::state::AppState;

use super::{
    GenerationProgress, MAX_CONTEXT_CHARS, MAX_SOAP_NOTE_CHARS, PATIENT_CTX_MAX_ITEM_CHARS,
    PATIENT_CTX_MAX_ITEMS_PER_LIST, format_progress_error,
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

        let recording = RecordingsRepo::get_by_id(&conn, &uuid)?;

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

/// Load the full `AppConfig` from the DB on a blocking thread, without a
/// recording. Used by commands that don't operate on a recording (e.g. the
/// standalone Letter Writer, which generates from OCR'd text).
pub(super) async fn load_config(db: &Arc<medical_db::Database>) -> AppResult<AppConfig> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)?;
        config.migrate();
        Ok::<_, AppError>(config)
    })
    .await
    .map_err(crate::commands::join_err)?
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
            AppError::AiProvider(
                "No AI provider configured. Check LM Studio / Ollama settings.".to_string(),
            )
        })
}

/// Persist a recording update on a blocking thread.
pub(super) async fn persist_recording(
    db: &Arc<medical_db::Database>,
    recording: Recording,
) -> AppResult<()> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::update(&conn, &recording).map_err(AppError::from)
    })
    .await
    .map_err(crate::commands::join_err)?
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
                },
            );
        }
    }

    result
}

/// Shared inner logic for document types generated from a SOAP note (referral,
/// letter, synopsis). Handles: preflight, resolve provider, validate SOAP note,
/// build completion request, call provider, strip markdown, check empty,
/// persist.
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
/// - `build_prompt`: closure `(soap_note, &settings) -> (system_prompt,
///   user_prompt)`.
/// - `set_field`: closure `(&mut Recording, String)` that assigns the
///   generated text to the right recording field.
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate_from_soap<F, S>(
    state: &AppState,
    recording: &mut Recording,
    settings: &GenerationSettings,
    config: &AppConfig,
    command_kind: medical_core::preflight::CommandKind,
    doc_type_label: &str,
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
        return Err(AppError::InvalidInput(format!(
            "SOAP note too large: {} chars, limit is {}",
            soap_note.len(),
            MAX_SOAP_NOTE_CHARS
        )));
    }

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

    let response = provider.complete(request).await.map_err(|e| match e {
        // Preserve EndpointOffline as-is so the frontend dialog can fire.
        AppError::EndpointOffline { .. } => e,
        // For other errors, keep the existing nicer wrapping.
        _ => AppError::AiProvider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;

    let text = document_generator::strip_markdown(&response.content);
    if text.trim().is_empty() {
        return Err(AppError::AiProvider(format!(
            "AI returned an empty {doc_type_label}."
        )));
    }

    set_field(recording, text.clone());
    Ok(text)
}

/// Parse a template string into the `SoapTemplate` enum.
pub(super) fn parse_soap_template(s: &str) -> SoapTemplate {
    match s.to_lowercase().as_str() {
        "new_patient" | "newpatient" => SoapTemplate::NewPatient,
        "telehealth" => SoapTemplate::Telehealth,
        "emergency" => SoapTemplate::Emergency,
        "pediatric" => SoapTemplate::Pediatric,
        "geriatric" => SoapTemplate::Geriatric,
        _ => SoapTemplate::FollowUp, // default
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
