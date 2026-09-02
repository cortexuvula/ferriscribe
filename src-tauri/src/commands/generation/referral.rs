//! `generate_referral` Tauri command — turns a recording's SOAP note into a referral.

use medical_core::error::AppResult;
use medical_processing::document_generator;

use crate::state::AppState;

use super::helpers::{
    generate_from_soap, load_recording_and_settings, persist_recording, run_generation_command,
};

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
    context: Option<String>,
) -> AppResult<String> {
    let recipient = recipient_type.unwrap_or_else(|| "Specialist".to_string());
    let urg = urgency.unwrap_or_else(|| "routine".to_string());
    let ctx = context.clone();

    run_generation_command(&app, &recording_id, "referral", context.as_deref(), async {
        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id).await?;

        let recipient = recipient.clone();
        let urg = urg.clone();
        let ctx2 = ctx.clone();
        let text = generate_from_soap(
            &state,
            Some(&app),
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
                    ctx2.as_deref(),
                )
            },
            |rec, text| {
                rec.referral = Some(text);
            },
        )
        .await?;

        persist_recording(&state.db, recording).await?;
        Ok(text)
    })
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
        // Drive the inner logic directly: load recording + settings, then run
        // the shared SOAP-based generator with a no-op prompt builder.
        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id)
                .await
                .unwrap();
        let result = generate_from_soap(
            &state,
            None, // app — no AppHandle in tests
            &mut recording,
            &settings,
            &config,
            medical_core::preflight::CommandKind::GenerateReferral,
            "referral letter",
            "referral",
            |soap_note, settings| {
                document_generator::build_referral_prompt(
                    soap_note,
                    "Specialist",
                    "routine",
                    settings.custom_referral_prompt.as_deref(),
                    None,
                )
            },
            |rec, text| {
                rec.referral = Some(text);
            },
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
    async fn generate_from_soap_records_referral_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
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

        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id)
                .await
                .unwrap();

        let text = generate_from_soap(
            &state,
            None, // app — no AppHandle in tests
            &mut recording,
            &settings,
            &config,
            medical_core::preflight::CommandKind::GenerateReferral,
            "referral letter",
            "referral",
            |soap_note, settings| {
                document_generator::build_referral_prompt(
                    soap_note,
                    "Specialist",
                    "routine",
                    settings.custom_referral_prompt.as_deref(),
                    None,
                )
            },
            |rec, text| {
                rec.referral = Some(text);
            },
        )
        .await
        .expect("referral generation succeeds");
        assert!(!text.is_empty());

        assert_eq!(
            recording.metadata["generation_stats"]["referral"]["completion_tokens"],
            serde_json::json!(64)
        );
        assert_eq!(
            recording.metadata["generation_stats"]["referral"]["model"],
            serde_json::json!("llama3")
        );
        assert!(recording.referral.is_some());
        assert_eq!(
            medical_core::types::recording::latest_tokens_per_second(&recording.metadata)
                .map(f64::is_finite),
            Some(true)
        );
    }
}
