//! Tauri commands for AI-powered document generation (SOAP, referral, letter, synopsis).

use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;
use tracing::{debug, info, error, instrument};
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::recording::Recording;
use medical_core::types::{CompletionRequest, Message, MessageContent, PatientContext, Role};
use medical_db::recordings::RecordingsRepo;
use medical_processing::document_generator;
use medical_processing::soap_generator::{self, SoapPromptConfig};
use medical_core::types::settings::SoapTemplate;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Input size bounds
// ---------------------------------------------------------------------------

/// Maximum size of a user-supplied `context` string (roughly ~12k tokens).
/// Prevents pasting multi-megabyte documents that would blow past provider
/// token limits or cause excessive memory usage.
const MAX_CONTEXT_CHARS: usize = 50_000;

/// Per-list item count cap on `PatientContext`. Generous against realistic
/// clinical input; exists to reject pathological payloads.
const PATIENT_CTX_MAX_ITEMS_PER_LIST: usize = 50;

/// Per-item character cap on `PatientContext` entries. A single med string
/// like "Lisinopril 10mg PO daily once in the morning with food" is well
/// under this; an entry over 500 chars is malformed input.
const PATIENT_CTX_MAX_ITEM_CHARS: usize = 500;

/// Maximum size of a recording transcript we will send to a provider.
/// A 2-hour transcript at ~150 wpm is ~180k chars; 500k gives comfortable
/// headroom while still catching obviously-corrupt or runaway inputs.
const MAX_TRANSCRIPT_CHARS: usize = 500_000;

/// Maximum size of a SOAP note re-fed into downstream document generation
/// (referral / letter / synopsis). SOAP notes are AI-generated, so this is
/// a sanity upper bound rather than an expected boundary.
const MAX_SOAP_NOTE_CHARS: usize = 500_000;

// ---------------------------------------------------------------------------
// Progress event payload
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct GenerationProgress {
    #[serde(rename = "type")]
    doc_type: String,
    status: String,
    recording_id: String,
}

/// Format an error for a `generation-progress` "failed" event. Falls back to a
/// kind-tagged placeholder when `unwrap_app_error_message_ref` returns an empty
/// or whitespace-only string, so the frontend never sees just `"failed: "`.
fn format_progress_error(err: &AppError) -> String {
    let msg = super::unwrap_app_error_message_ref(err);
    let trimmed = msg.trim();
    if trimmed.is_empty() {
        format!("failed: unknown error ({})", err.kind_str())
    } else {
        format!("failed: {}", trimmed)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Loaded settings needed for generation.
struct GenerationSettings {
    model: String,
    temperature: f32,
    icd_version: String,
    ai_provider: String,
    custom_soap_prompt: Option<String>,
    custom_referral_prompt: Option<String>,
    custom_letter_prompt: Option<String>,
    custom_synopsis_prompt: Option<String>,
}

/// Load a recording and settings from DB on a blocking thread.
///
/// All rusqlite work is offloaded via `spawn_blocking` so we never block the
/// Tokio async runtime.
async fn load_recording_and_settings(
    db: &Arc<medical_db::Database>,
    recording_id: &str,
) -> AppResult<(Recording, GenerationSettings)> {
    let uuid = Uuid::parse_str(recording_id)
        .map_err(|e| AppError::Other(format!("Invalid recording ID: {e}")))?;
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;

        let recording = RecordingsRepo::get_by_id(&conn, &uuid)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let config = medical_db::settings::SettingsRepo::load_config(&conn)
            .ok()
            .map(|mut c| { c.migrate(); c });
        let settings = match config {
            Some(cfg) => {
                let icd = match cfg.icd_version {
                    medical_core::types::settings::IcdVersion::Icd9 => "ICD-9".to_string(),
                    medical_core::types::settings::IcdVersion::Icd10 => "ICD-10".to_string(),
                    medical_core::types::settings::IcdVersion::Both => "both".to_string(),
                };
                GenerationSettings {
                    model: cfg.ai_model,
                    temperature: cfg.temperature,
                    icd_version: icd,
                    ai_provider: cfg.ai_provider,
                    custom_soap_prompt: cfg.custom_soap_prompt,
                    custom_referral_prompt: cfg.custom_referral_prompt,
                    custom_letter_prompt: cfg.custom_letter_prompt,
                    custom_synopsis_prompt: cfg.custom_synopsis_prompt,
                }
            }
            None => GenerationSettings {
                model: String::new(),
                temperature: 0.2,
                icd_version: "ICD-10".to_string(),
                ai_provider: "lmstudio".to_string(),
                custom_soap_prompt: None,
                custom_referral_prompt: None,
                custom_letter_prompt: None,
                custom_synopsis_prompt: None,
            },
        };

        Ok::<_, AppError>((recording, settings))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Resolve the AI provider from the registry using the settings provider name.
async fn resolve_provider(
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
async fn persist_recording(
    db: &Arc<medical_db::Database>,
    recording: Recording,
) -> AppResult<()> {
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        RecordingsRepo::update(&conn, &recording).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Parse a template string into the `SoapTemplate` enum.
fn parse_soap_template(s: &str) -> SoapTemplate {
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
fn build_completion_request(
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
pub(super) fn validate_patient_context(pc: &PatientContext) -> AppResult<()> {
    let lists: [(&str, &[String]); 3] = [
        ("medications", &pc.medications),
        ("allergies", &pc.allergies),
        ("conditions", &pc.conditions),
    ];

    let mut total: usize = 0;
    for (label, items) in lists {
        if items.len() > PATIENT_CTX_MAX_ITEMS_PER_LIST {
            return Err(AppError::Other(format!(
                "Too many {label} entries: {} (limit is {})",
                items.len(),
                PATIENT_CTX_MAX_ITEMS_PER_LIST
            )));
        }
        for item in items {
            if item.len() > PATIENT_CTX_MAX_ITEM_CHARS {
                return Err(AppError::Other(format!(
                    "Patient context entry too long in {label}: {} chars (limit is {})",
                    item.len(),
                    PATIENT_CTX_MAX_ITEM_CHARS
                )));
            }
            total += item.len();
        }
    }

    if total > MAX_CONTEXT_CHARS {
        return Err(AppError::Other(format!(
            "Patient context too large: {total} chars (limit is {MAX_CONTEXT_CHARS})"
        )));
    }

    Ok(())
}

/// True iff every surfaced list (`medications`, `allergies`, `conditions`)
/// is empty. Such a payload contributes nothing to the prompt and must not
/// be persisted, so the recording metadata stays clean.
fn patient_context_is_empty(pc: &PatientContext) -> bool {
    pc.medications.is_empty() && pc.allergies.is_empty() && pc.conditions.is_empty()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

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
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", super::unwrap_app_error_message(e))))?;

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

    Ok(soap_text)
}

/// Generate a referral letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "referral"`.
#[tauri::command]
pub async fn generate_referral(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    recipient_type: Option<String>,
    urgency: Option<String>,
) -> AppResult<String> {
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "referral".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result = generate_referral_inner(
        &state,
        &recording_id,
        recipient_type.as_deref(),
        urgency.as_deref(),
    )
    .await;

    match &result {
        Ok(_) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "referral".into(),
                    status: "completed".into(),
                    recording_id: recording_id.clone(),
                },
            );
        }
        Err(err) => {
            let _ = app.emit(
                "generation-progress",
                GenerationProgress {
                    doc_type: "referral".into(),
                    status: format_progress_error(err),
                    recording_id: recording_id.clone(),
                },
            );
        }
    }

    result
}

async fn generate_referral_inner(
    state: &AppState,
    recording_id: &str,
    recipient_type: Option<&str>,
    urgency: Option<&str>,
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, recording_id).await?;
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

    let recipient = recipient_type.unwrap_or("Specialist");
    let urg = urgency.unwrap_or("routine");

    let (system_prompt, user_prompt) =
        document_generator::build_referral_prompt(soap_note, recipient, urg, settings.custom_referral_prompt.as_deref());

    debug!(
        "generate_referral: provider='{}', recording='{}', recipient='{}', urgency='{}'",
        provider.name(),
        recording_id,
        recipient,
        urg,
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
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", super::unwrap_app_error_message(e))))?;

    let referral_text = response.content;
    if referral_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty referral letter.".to_string(),
        ));
    }

    // Persist to DB (on blocking thread)
    recording.referral = Some(referral_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(referral_text)
}

/// Generate a patient letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "letter"`.
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
) -> AppResult<String> {
    let _ = app.emit(
        "generation-progress",
        GenerationProgress {
            doc_type: "letter".into(),
            status: "started".into(),
            recording_id: recording_id.clone(),
        },
    );

    let result =
        generate_letter_inner(&state, &recording_id, letter_type.as_deref()).await;

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
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, recording_id).await?;
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

    let (system_prompt, user_prompt) =
        document_generator::build_letter_prompt(soap_note, ltype, settings.custom_letter_prompt.as_deref());

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

    let response = provider
        .complete(request)
        .await
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", super::unwrap_app_error_message(e))))?;

    let letter_text = response.content;
    if letter_text.is_empty() {
        return Err(AppError::AiProvider(
            "AI returned an empty letter.".to_string(),
        ));
    }

    // Persist to DB (on blocking thread)
    recording.letter = Some(letter_text.clone());
    persist_recording(&state.db, recording).await?;

    Ok(letter_text)
}

/// Generate a brief synopsis from a recording's SOAP note.
///
/// The synopsis is returned directly and stored in the recording's metadata
/// (the `Recording` struct does not have a dedicated `synopsis` field).
#[tauri::command]
pub async fn generate_synopsis(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    let (mut recording, settings) =
        load_recording_and_settings(&state.db, &recording_id).await?;
    let provider = resolve_provider(&state, &settings.ai_provider).await?;

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

    let (system_prompt, user_prompt) = document_generator::build_synopsis_prompt(soap_note, settings.custom_synopsis_prompt.as_deref());

    debug!(
        "generate_synopsis: provider='{}', recording='{}'",
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

    let response = provider
        .complete(request)
        .await
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", super::unwrap_app_error_message(e))))?;

    let synopsis_text = response.content;
    if synopsis_text.is_empty() {
        return Err(AppError::AiProvider(
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
    persist_recording(&state.db, recording).await?;

    Ok(synopsis_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::PatientContext;

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
        let fifty: Vec<&str> = std::iter::repeat(big.as_str()).take(50).collect();
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
