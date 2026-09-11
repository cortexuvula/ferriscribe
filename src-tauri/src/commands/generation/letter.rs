//! `generate_letter` Tauri command — turns a recording's SOAP note into a patient letter.

use medical_core::error::AppResult;
use medical_core::types::PatientContext;
use medical_db::LetterAudiencesRepo;
use medical_processing::document_generator::{self, LetterAudienceContext};
use uuid::Uuid;

use crate::state::AppState;

use super::super::specialty::resolve_specialty_prompt;
use super::helpers::{
    acquire_generation_lock, context_metadata_patch, ensure_prompt_within_cap,
    fold_structured_context, fresh_stats_patch, generate_from_soap, load_recording_and_settings,
    persist_producer_patch, persist_provenance, run_generation_command, validate_patient_context,
};

use medical_processing::specialty::DocType as PackDocType;

/// Generate a patient letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "letter"`.
///
/// F2: structured patient context arrives as `patient_context` and is
/// folded into the prompt context HERE (Rust), not by the caller — the
/// fold is shared with the freshness digest, so it can never drift.
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
    audience_id: Option<Uuid>,
    context: Option<String>,
    patient_context: Option<PatientContext>,
) -> AppResult<String> {
    // One generation per recording at a time (see acquire_generation_lock).
    let _generation_lock = acquire_generation_lock(&state, &recording_id)?;

    if let Some(ref pc) = patient_context {
        validate_patient_context(pc)?;
    }

    let ltype = letter_type
        .clone()
        .unwrap_or_else(|| "follow-up".to_string());
    // F2 Rust fold: structured lists + freeform notes/OCR, one string.
    let folded = fold_structured_context(patient_context.as_ref(), context.as_deref());

    let inner = generate_letter_inner(
        &state,
        Some(&app),
        &recording_id,
        letter_type.as_deref(),
        audience_id,
        ltype,
        context.as_deref(),
        patient_context.as_ref(),
        folded.clone(),
    );
    run_generation_command(&app, &recording_id, "letter", folded.as_deref(), inner).await
}

/// Inner body of [`generate_letter`], callable without an `AppHandle`
/// (tests pass `None` — progress events are skipped; everything else,
/// including the provenance write, runs identically).
#[allow(clippy::too_many_arguments)]
async fn generate_letter_inner(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording_id: &str,
    letter_type: Option<&str>,
    audience_id: Option<Uuid>,
    ltype: String,
    raw_context: Option<&str>,
    patient_context: Option<&PatientContext>,
    folded: Option<String>,
) -> AppResult<String> {
    // Single DB load: settings + recording + config. The audience lookup
    // runs after the load, matching the original inner-function ordering,
    // and uses the same DB connection.
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Same generation-time cap every custom prompt gets — covers configs
    // that arrived via sync.
    ensure_prompt_within_cap(settings.custom_letter_prompt.as_deref(), "letter")?;
    let specialty_body = resolve_specialty_prompt(state, &config, PackDocType::Letter).await?;

    // Audience lookup on a blocking worker — rusqlite never runs on the
    // async runtime (the invariant every other generation DB access
    // follows; this one predates the helper layer).
    let audience_context: Option<LetterAudienceContext> = match audience_id {
        Some(id) => {
            let db = std::sync::Arc::clone(&state.db);
            let audience = tokio::task::spawn_blocking(move || -> AppResult<_> {
                let conn = db.conn()?;
                Ok(LetterAudiencesRepo::get_by_id(&conn, &id)?)
            })
            .await
            .map_err(crate::commands::join_err)??;
            Some(LetterAudienceContext {
                name: audience.name,
                system_prompt: audience.system_prompt,
                user_template: audience.user_template,
            })
        }
        None => None,
    };

    let lt = ltype;
    let aud = audience_context;
    let ctx2 = folded.clone();
    let text = generate_from_soap(
        state,
        app,
        &mut recording,
        &settings,
        &config,
        medical_core::preflight::CommandKind::GenerateLetter,
        "letter",
        "letter",
        move |soap_note, settings| {
            document_generator::build_letter_prompt(
                soap_note,
                &lt,
                aud.as_ref(),
                settings.custom_letter_prompt.as_deref(),
                specialty_body.as_deref(),
                ctx2.as_deref(),
            )
        },
        |rec, text| {
            rec.letter = Some(text);
        },
    )
    .await?;

    // Column-scoped persist: the recording snapshot is stale by however
    // long the LLM ran — a whole-row update would revert concurrent
    // column writes (editor saves, another generator's output).
    // F3: context + patient_context mirror with uniform null-clearing.
    let mut metadata_patch = fresh_stats_patch(&recording, "letter");
    metadata_patch.extend(context_metadata_patch(raw_context, patient_context));
    persist_producer_patch(
        state,
        recording.id,
        medical_db::recordings::ProducerPersist {
            letter: Some(text.clone()),
            metadata_patch,
            ..Default::default()
        },
    )
    .await?;

    // Freshness provenance (best-effort): effective input digest with the
    // same fold the prompt used, plus the SOAP source binding —
    // freshness compares it against digest(recordings.soap_note).
    let source_digest = recording
        .soap_note
        .as_deref()
        .map(super::freshness::digest_str);
    let input_digest =
        super::freshness::letter_input_digest(folded.as_deref(), letter_type, audience_id, &config);
    persist_provenance(
        state,
        recording.id,
        "letter",
        settings.ai_provider.clone(),
        settings.model.clone(),
        input_digest,
        text.clone(),
        source_digest,
    )
    .await;

    Ok(text)
}


#[cfg(test)]
pub(crate) async fn generate_letter_inner_for_test(
    state: &AppState,
    recording_id: &str,
    inputs: &super::freshness::CurrentDocInputs,
) -> AppResult<String> {
    let folded = super::helpers::fold_structured_context(
        inputs.patient_context.as_ref(),
        inputs.context.as_deref(),
    );
    let ltype = inputs.letter_type.clone().unwrap_or_else(|| "follow-up".to_string());
    generate_letter_inner(
        state,
        None,
        recording_id,
        inputs.letter_type.as_deref(),
        inputs.audience_id,
        ltype,
        inputs.context.as_deref(),
        inputs.patient_context.as_ref(),
        folded,
    )
    .await
}

#[cfg(test)]
mod preflight_tests {
    use super::super::test_helpers::{assert_endpoint_offline, build_test_state_with_recording};
    use super::*;
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_letter_returns_endpoint_offline_when_ai_unreachable() {
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
        let result = generate_letter_inner(
            &state,
            None, // app — no AppHandle in tests
            &recording_id,
            None,
            None,
            "follow-up".to_string(),
            None,
            None,
            None,
        )
        .await;
        assert_endpoint_offline(result, "Ollama", start);
    }
}
