//! `generate_referral` Tauri command — turns a recording's SOAP note into a referral.

use medical_core::error::AppResult;
use medical_core::types::PatientContext;
use medical_processing::document_generator;

use crate::state::AppState;

use super::super::specialty::resolve_specialty_prompt;
use super::helpers::{
    acquire_generation_lock, context_metadata_patch, ensure_prompt_within_cap,
    fold_structured_context, fresh_stats_patch, generate_from_soap, load_recording_and_settings,
    persist_producer_patch, persist_provenance, run_generation_command, validate_patient_context,
};

use medical_processing::specialty::DocType as PackDocType;

/// Generate a referral letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "referral"`.
///
/// F2: structured patient context arrives as `patient_context` and is
/// folded into the prompt context HERE (Rust), not by the caller — the
/// fold is shared with the freshness digest, so it can never drift.
#[tauri::command]
pub async fn generate_referral(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    recipient_type: Option<String>,
    urgency: Option<String>,
    context: Option<String>,
    patient_context: Option<PatientContext>,
) -> AppResult<String> {
    // One generation per recording at a time (see acquire_generation_lock).
    let _generation_lock = acquire_generation_lock(&state, &recording_id)?;

    if let Some(ref pc) = patient_context {
        validate_patient_context(pc)?;
    }

    // F2 Rust fold: structured lists + freeform notes/OCR, one string.
    let folded = fold_structured_context(patient_context.as_ref(), context.as_deref());

    let inner = generate_referral_inner(
        &state,
        Some(&app),
        &recording_id,
        recipient_type.as_deref(),
        urgency.as_deref(),
        context.as_deref(),
        patient_context.as_ref(),
        folded.clone(),
    );
    run_generation_command(&app, &recording_id, "referral", folded.as_deref(), inner).await
}

/// Inner body of [`generate_referral`], callable without an `AppHandle`
/// (tests pass `None` — progress events are skipped; everything else,
/// including the provenance write, runs identically).
#[allow(clippy::too_many_arguments)]
async fn generate_referral_inner(
    state: &AppState,
    app: Option<&tauri::AppHandle>,
    recording_id: &str,
    recipient_type: Option<&str>,
    urgency: Option<&str>,
    raw_context: Option<&str>,
    patient_context: Option<&PatientContext>,
    folded: Option<String>,
) -> AppResult<String> {
    let (mut recording, settings, config) =
        load_recording_and_settings(&state.db, recording_id).await?;

    // Same generation-time cap every custom prompt gets — covers configs
    // that arrived via sync.
    ensure_prompt_within_cap(settings.custom_referral_prompt.as_deref(), "referral")?;
    let specialty_body = resolve_specialty_prompt(state, &config, PackDocType::Referral).await?;

    let recipient = recipient_type.unwrap_or("Specialist").to_string();
    let urg = urgency.unwrap_or("routine").to_string();
    let ctx2 = folded.clone();
    let text = generate_from_soap(
        state,
        app,
        &mut recording,
        &settings,
        &config,
        medical_core::preflight::CommandKind::GenerateReferral,
        "referral letter",
        "referral",
        move |soap_note, settings| {
            document_generator::build_referral_prompt(
                soap_note,
                &recipient,
                &urg,
                settings.custom_referral_prompt.as_deref(),
                specialty_body.as_deref(),
                ctx2.as_deref(),
            )
        },
        |rec, text| {
            rec.referral = Some(text);
        },
    )
    .await?;

    // Column-scoped persist: the recording snapshot is stale by however
    // long the LLM ran — a whole-row update would revert concurrent
    // column writes (editor saves, another generator's output).
    // F3: context + patient_context mirror with uniform null-clearing.
    let mut metadata_patch = fresh_stats_patch(&recording, "referral");
    metadata_patch.extend(context_metadata_patch(raw_context, patient_context));
    persist_producer_patch(
        state,
        recording.id,
        medical_db::recordings::ProducerPersist {
            referral: Some(text.clone()),
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
    let input_digest = super::freshness::referral_input_digest(
        folded.as_deref(),
        recipient_type,
        urgency,
        &config,
    );
    persist_provenance(
        state,
        recording.id,
        "referral",
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
pub(crate) async fn generate_referral_inner_for_test(
    state: &AppState,
    recording_id: &str,
    inputs: &super::freshness::CurrentDocInputs,
) -> AppResult<String> {
    let folded = super::helpers::fold_structured_context(
        inputs.patient_context.as_ref(),
        inputs.context.as_deref(),
    );
    generate_referral_inner(
        state,
        None,
        recording_id,
        inputs.recipient_type.as_deref(),
        inputs.urgency.as_deref(),
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
    async fn generate_referral_returns_endpoint_offline_when_ai_unreachable() {
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
        let result = generate_referral_inner(
            &state,
            None, // app — no AppHandle in tests
            &recording_id,
            None,
            None,
            None,
            None,
            None,
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
    async fn generate_referral_records_stats_and_provenance() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        // Loopback → preflight probe is skipped; the mock serves completions.
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Dear Cardiology, please assess this patient for chest pain.",
            64,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        // generate_from_soap requires an existing SOAP note.
        {
            let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
            let conn = state.db.conn().expect("conn");
            let mut rec =
                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
            rec.soap_note = Some("S: Chest pain.\nA: Angina.\nP: Cardiology referral.".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }

        let pc = PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: vec!["Synthetic medication".into()],
            allergies: vec![],
            conditions: vec![],
        };
        let text = generate_referral_inner(
            &state,
            None,
            &recording_id,
            None,
            None,
            Some("Synthetic note"),
            Some(&pc),
            fold_structured_context(Some(&pc), Some("Synthetic note")),
        )
        .await
        .expect("referral generation succeeds");
        assert!(!text.is_empty());

        // Provenance row written with the folded-context input digest.
        let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let prov = medical_db::generation_provenance::GenerationProvenanceRepo::get(
            &conn, uuid, "referral",
        )
        .expect("provenance read")
        .expect("provenance row exists");
        assert_eq!(
            prov.output_digest,
            super::super::freshness::digest_str(&text)
        );
        assert!(
            prov.source_digest.is_some(),
            "referral binds its SOAP source"
        );
    }
}
