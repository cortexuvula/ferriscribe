//! Shared helpers for the four generation commands.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::recording::Recording;
use medical_core::types::settings::{AppConfig, SoapTemplate};
use medical_core::types::{CompletionRequest, Message, MessageContent, PatientContext, Role};
use medical_db::recordings::RecordingsRepo;
use uuid::Uuid;

use crate::state::AppState;

use super::{MAX_CONTEXT_CHARS, PATIENT_CTX_MAX_ITEM_CHARS, PATIENT_CTX_MAX_ITEMS_PER_LIST};

/// Loaded settings needed for generation.
pub(super) struct GenerationSettings {
    pub model: String,
    pub temperature: f32,
    pub icd_version: String,
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
        .map_err(|e| AppError::Other(format!("Invalid recording ID: {e}")))?;
    let db = Arc::clone(db);

    tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;

        let recording = RecordingsRepo::get_by_id(&conn, &uuid)
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Database(e.to_string()))?;
        config.migrate();

        let icd = match config.icd_version {
            medical_core::types::settings::IcdVersion::Icd9 => "ICD-9".to_string(),
            medical_core::types::settings::IcdVersion::Icd10 => "ICD-10".to_string(),
            medical_core::types::settings::IcdVersion::Both => "both".to_string(),
        };
        let settings = GenerationSettings {
            model: config.ai_model.clone(),
            temperature: config.temperature,
            icd_version: icd,
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
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Resolve the AI provider from the registry using the settings provider name.
pub(super) async fn resolve_provider(
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
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        RecordingsRepo::update(&conn, &recording).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
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
