//! `generate_letter` Tauri command — turns a recording's SOAP note into a patient letter.

use medical_core::error::AppResult;
use medical_db::LetterAudiencesRepo;
use medical_processing::document_generator::{self, LetterAudienceContext};
use uuid::Uuid;

use crate::state::AppState;

use super::helpers::{
    generate_from_soap, load_recording_and_settings, persist_recording, run_generation_command,
};

/// Generate a patient letter from a recording's SOAP note.
///
/// Emits `generation-progress` events with `type: "letter"`.
#[tauri::command]
pub async fn generate_letter(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    letter_type: Option<String>,
    audience_id: Option<Uuid>,
    context: Option<String>,
) -> AppResult<String> {
    let ltype = letter_type.unwrap_or_else(|| "follow-up".to_string());
    let ctx = context.clone();

    run_generation_command(&app, &recording_id, "letter", context.as_deref(), async {
        // Single DB load: settings + recording + config. The audience lookup
        // runs after the load, matching the original inner-function ordering,
        // and uses the same DB connection.
        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id).await?;

        let audience_context: Option<LetterAudienceContext> = match audience_id {
            Some(id) => {
                let conn = state.db.conn()?;
                let audience = LetterAudiencesRepo::get_by_id(&conn, &id)?;
                Some(LetterAudienceContext {
                    name: audience.name,
                    system_prompt: audience.system_prompt,
                    user_template: audience.user_template,
                })
            }
            None => None,
        };

        let lt = ltype.clone();
        let aud = audience_context;
        let ctx2 = ctx.clone();
        let text = generate_from_soap(
            &state,
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
                    ctx2.as_deref(),
                )
            },
            |rec, text| {
                rec.letter = Some(text);
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
    use super::super::test_helpers::build_test_state_with_recording;
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
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
        // Drive the inner logic directly: load recording + settings, then run
        // the shared SOAP-based generator with a no-op prompt builder.
        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id)
                .await
                .unwrap();
        let result = generate_from_soap(
            &state,
            &mut recording,
            &settings,
            &config,
            medical_core::preflight::CommandKind::GenerateLetter,
            "letter",
            "letter",
            |soap_note, settings| {
                document_generator::build_letter_prompt(
                    soap_note,
                    "follow-up",
                    None,
                    settings.custom_letter_prompt.as_deref(),
                    None,
                )
            },
            |rec, text| {
                rec.letter = Some(text);
            },
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

        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "should have short-circuited at ~3s; took {elapsed:?}"
        );
    }
}
