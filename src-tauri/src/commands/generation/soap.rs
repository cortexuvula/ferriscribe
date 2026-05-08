//! `generate_soap` Tauri command — turns a recording's transcript into a SOAP note.

use medical_core::error::{AppError, AppResult};
use medical_core::types::PatientContext;
use medical_processing::soap_generator::{self, SoapPromptConfig};
use tauri::Emitter;
use tracing::{debug, error, info, instrument};

use crate::state::AppState;

use super::helpers::{
    build_completion_request, load_recording_and_settings, parse_soap_template,
    patient_context_is_empty, persist_recording, resolve_provider, validate_patient_context,
};
use super::{format_progress_error, GenerationProgress, MAX_CONTEXT_CHARS, MAX_TRANSCRIPT_CHARS};

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
        .map_err(|e| AppError::AiProvider(format!("AI completion failed: {}", crate::commands::unwrap_app_error_message(e))))?;

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
