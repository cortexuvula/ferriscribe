//! Tauri commands for transcription. The heavy lifting lives in [`inner`];
//! [`helpers`] holds the audio-loading, hallucination-detection, persistence,
//! and failure-bookkeeping utilities (and their tests).

use tracing::instrument;

use medical_core::error::AppResult;

use crate::state::AppState;

pub(crate) mod helpers;
mod inner;

// `transcribe_recording_inner` is a regular async fn (not a Tauri command),
// re-exported for `commands::pipeline:97` which calls it directly with a
// cancel token.
pub use inner::transcribe_recording_inner;

/// Transcribe a previously recorded WAV file using the local STT provider.
///
/// Emits `transcription-progress` events ("loading", "transcribing", "complete")
/// so the frontend can display live status.  Returns the transcript text on success.
#[tauri::command]
#[instrument(skip(app, state), fields(recording_id = %recording_id))]
pub async fn transcribe_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    language: Option<String>,
    diarize: Option<bool>,
) -> AppResult<String> {
    // The #[tauri::command] wrapper: frontend callers can't supply a cancel
    // token, so we pass `None`. Pipeline callers should invoke
    // `transcribe_recording_inner` directly with `Some(token)` instead.
    transcribe_recording_inner(app, state, recording_id, language, diarize, None).await
}

#[tauri::command]
pub async fn list_stt_providers(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<(String, bool)>> {
    let guard = state.stt_providers.lock().await;
    match guard.as_ref() {
        Some(provider) => Ok(vec![(provider.name().to_string(), true)]),
        None => Ok(vec![]),
    }
}
