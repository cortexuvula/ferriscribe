//! Background pipeline: transcribe → generate SOAP in one command.

use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::PatientContext;
use medical_db::recordings::RecordingsRepo;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
struct PipelineProgress {
    recording_id: String,
    stage: String,
    error: Option<String>,
}

/// Run the full transcribe → SOAP pipeline for a recording.
///
/// This command is designed to be called fire-and-forget from the frontend.
/// Progress is reported via `pipeline-progress` events so the frontend can
/// track multiple concurrent pipelines by recording ID.
#[tauri::command]
#[instrument(skip(app, state, context), fields(recording_id = %recording_id))]
pub async fn process_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    context: Option<String>,
    template: Option<String>,
    patient_context: Option<PatientContext>,
) -> AppResult<String> {
    info!(
        has_context = context.is_some(),
        has_patient_context = patient_context.is_some(),
        template = template.as_deref().unwrap_or("default"),
        "Pipeline started: transcribe → SOAP"
    );

    if let Some(ref pc) = patient_context {
        super::generation::validate_patient_context(pc)?;
    }
    // Validate context size up-front (fail fast before transcription runs).
    if let Some(ref ctx) = context
        && ctx.len() > super::generation::MAX_CONTEXT_CHARS
    {
        return Err(AppError::InvalidInput(format!(
            "Context too large: {} chars, limit is {}",
            ctx.len(),
            super::generation::MAX_CONTEXT_CHARS
        )));
    }

    let rid = recording_id.clone();

    // Register a cancel token so the frontend can ask us to bail between
    // stages and interrupt in-flight provider work.
    let cancel = CancellationToken::new();
    {
        let mut guard = state
            .pipeline_cancels
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("pipeline_cancels: {e}")))?;
        guard.insert(recording_id.clone(), cancel.clone());
    }
    // Ensure the flag is removed on every exit path.
    let _cancel_guard = CancelGuard {
        cancels: Arc::clone(&state.pipeline_cancels),
        key: recording_id.clone(),
    };

    // If no explicit template, read the user's preferred template from settings.
    let template = match template {
        Some(t) => Some(t),
        None => {
            let db = std::sync::Arc::clone(&state.db);
            tokio::task::spawn_blocking(move || {
                let conn = db.conn().map_err(|e| {
                    tracing::warn!(error = %e, "could not get DB conn for SOAP template lookup");
                    e
                }).ok()?;
                let mut cfg = medical_db::settings::SettingsRepo::load_config(&conn).map_err(|e| {
                    tracing::warn!(error = %e, "could not load config for SOAP template lookup");
                    e
                }).ok()?;
                cfg.migrate();
                let t = match cfg.soap_template {
                    medical_core::types::settings::SoapTemplate::FollowUp => "follow_up",
                    medical_core::types::settings::SoapTemplate::NewPatient => "new_patient",
                    medical_core::types::settings::SoapTemplate::Telehealth => "telehealth",
                    medical_core::types::settings::SoapTemplate::Emergency => "emergency",
                    medical_core::types::settings::SoapTemplate::Pediatric => "pediatric",
                    medical_core::types::settings::SoapTemplate::Geriatric => "geriatric",
                };
                Some(t.to_string())
            })
            .await
            .ok()
            .flatten()
        }
    };

    // --- Stage 1: Transcribe ---
    emit_progress(&app, &rid, "transcribing", None);

    // Forward the pipeline cancel flag through to the transcription inner
    // helper so the user's cancel click can interrupt transcription at the
    // STT-call boundary instead of waiting for the whole (often 30s+) pass
    // to finish.
    let transcript_result = super::transcription::transcribe_recording_inner(
        app.clone(),
        state.clone(),
        recording_id.clone(),
        None,        // language — use default
        Some(false), // diarize disabled — raw transcript is cleaner for clinical use
        Some(cancel.clone()),
    )
    .await;

    if let Err(e) = transcript_result {
        let msg = e.to_string();
        error!(error = %msg, "Pipeline failed at transcription stage");
        emit_progress(&app, &rid, "failed", Some(msg));
        // Return the original typed error so the frontend receives a
        // structured {kind, message} payload instead of a plain string.
        return Err(e);
    }

    info!("Pipeline stage 1 complete: transcription succeeded");

    if cancel.is_cancelled() {
        let msg = "Pipeline cancelled by user after transcription".to_string();
        warn!("{msg}");
        emit_progress(&app, &rid, "failed", Some(msg.clone()));
        return Err(AppError::Cancelled);
    }

    // --- Stage 2: Generate SOAP ---
    emit_progress(&app, &rid, "generating_soap", None);

    // generate_soap doesn't take a CancellationToken — race it against the
    // cancel signal so a click during SOAP generation drops the in-flight
    // reqwest future (which closes the upstream TCP connection at the
    // socket layer) instead of waiting for the AI to finish. With `biased;`
    // the cancel branch is checked first on every poll so a fast cancel
    // doesn't lose to the completion path.
    let soap_result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let msg = "Pipeline cancelled by user during SOAP generation".to_string();
            warn!("{msg}");
            emit_progress(&app, &rid, "failed", Some(msg));
            return Err(AppError::Cancelled);
        }
        res = super::generation::soap::generate_soap(
            app.clone(),
            state.clone(),
            recording_id.clone(),
            template,
            context,
            patient_context,
        ) => res,
    };

    match soap_result {
        Ok(soap_text) => {
            // Fetch the recording name for the notification
            let display_name = get_recording_display_name(&state, &recording_id).await;
            emit_progress(&app, &rid, "completed", None);

            info!(
                soap_len = soap_text.len(),
                "Pipeline complete: transcription + SOAP generation succeeded"
            );

            // Emit a dedicated notification event for the toast
            let _ = app.emit(
                "pipeline-complete",
                serde_json::json!({
                    "recording_id": rid,
                    "display_name": display_name,
                }),
            );

            Ok(soap_text)
        }
        Err(e) => {
            let msg = e.to_string();
            error!(error = %msg, "Pipeline failed at SOAP generation stage");
            emit_progress(&app, &rid, "failed", Some(msg));
            Err(e)
        }
    }
}

fn emit_progress(app: &tauri::AppHandle, recording_id: &str, stage: &str, error: Option<String>) {
    let _ = app.emit(
        "pipeline-progress",
        PipelineProgress {
            recording_id: recording_id.to_string(),
            stage: stage.to_string(),
            error,
        },
    );
}

async fn get_recording_display_name(state: &AppState, recording_id: &str) -> String {
    let uuid = match Uuid::parse_str(recording_id) {
        Ok(u) => u,
        Err(_) => return "Recording".to_string(),
    };
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db
            .conn()
            .map_err(|e| {
                tracing::warn!(error = %e, "could not get DB conn for display name lookup");
                e
            })
            .ok()?;
        let rec = RecordingsRepo::get_by_id(&conn, &uuid)
            .map_err(|e| {
                tracing::warn!(error = %e, "could not load recording for display name lookup");
                e
            })
            .ok()?;
        Some(rec.patient_name.unwrap_or(rec.filename))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "Recording".to_string())
}

/// RAII helper: remove the cancel token from the map when the pipeline exits
/// (success, error, or panic unwind) so we don't leak entries.
struct CancelGuard {
    cancels: Arc<std::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
    key: String,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        match self.cancels.lock() {
            Ok(mut guard) => {
                guard.remove(&self.key);
            }
            Err(poisoned) => {
                tracing::error!(
                    key = %self.key,
                    "Pipeline cancel-token map is poisoned; entry leaked. \
                     Subsequent pipeline runs for this recording_id may behave incorrectly."
                );
                // Best-effort cleanup using the poisoned inner.
                let mut inner = poisoned.into_inner();
                inner.remove(&self.key);
            }
        }
    }
}

/// Cancel an in-progress pipeline run for the given recording_id.
///
/// Looks up the recording's `CancellationToken` in the registry and fires it.
/// Effects:
/// - Remote STT provider: aborts the in-flight HTTP request via `tokio::select!`
///   on `cancel.cancelled()` (drops the reqwest future, tears down the TCP
///   connection).
/// - Local STT provider: returns `AppError::Cancelled` before model load if the
///   token is already cancelled, or after the blocking whisper pass returns
///   if the user cancelled mid-pass. Cannot interrupt whisper-rs mid-pass
///   (would require callback plumbing).
/// - Stage transitions: the pipeline checks the token between transcription
///   and SOAP generation and bails if cancelled.
#[tauri::command]
pub fn cancel_pipeline(state: tauri::State<'_, AppState>, recording_id: String) -> AppResult<bool> {
    let guard = state
        .pipeline_cancels
        .lock()
        .map_err(|_| AppError::Other("pipeline_cancels mutex poisoned".to_string()))?;
    if let Some(token) = guard.get(&recording_id) {
        token.cancel();
        info!(recording_id = %recording_id, "Pipeline cancel requested");
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use medical_core::types::PatientContext;

    /// Tests at the call-site level: process_recording must invoke
    /// validate_patient_context and surface its Err for an oversized payload.
    /// The validator's own behavior is covered in commands::generation::tests.
    #[test]
    fn validate_patient_context_call_rejects_oversized_item() {
        let long = "y".repeat(501);
        let pc = PatientContext {
            patient_name: None,
            prior_soap_notes: vec![],
            medications: vec![long],
            allergies: vec![],
            conditions: vec![],
        };
        let result = super::super::generation::validate_patient_context(&pc);
        assert!(result.is_err(), "expected Err on 501-char medication");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("too long") || msg.contains("500"),
            "expected too-long error, got: {msg}"
        );
    }
}
