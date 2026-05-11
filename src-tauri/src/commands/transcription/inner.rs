//! `transcribe_recording_inner` — the cancel-aware body of [`super::transcribe_recording`],
//! plus the speaker-attributed transcript formatter it relies on.

use std::sync::Arc;

use chrono::Utc;
use tauri::{Emitter, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::recording::ProcessingStatus;
use medical_core::types::stt::SttConfig;
use medical_db::recordings::RecordingsRepo;
use medical_db::settings::SettingsRepo;
use medical_db::vocabulary::VocabularyRepo;
use medical_processing::vocabulary_corrector;

use crate::commands::unwrap_app_error_message;
use crate::state::AppState;

use super::helpers::{
    is_repeated_phrase_hallucination, load_wav_to_audio_data, mark_recording_failed,
    mark_recording_failed_db_only, persist_orphaned_transcript,
};

/// Inner implementation of [`super::transcribe_recording`] that accepts an
/// optional cancel token. The token is forwarded into the STT provider's
/// `transcribe` call so providers that support in-flight cancellation
/// (e.g. the remote HTTP provider) can abort immediately; in addition we keep
/// the checkpoint-based bail-outs at stage boundaries (before the STT call and
/// before vocabulary correction) so callers without a real in-flight hook
/// still cancel promptly between stages.
pub async fn transcribe_recording_inner(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    recording_id: String,
    language: Option<String>,
    diarize: Option<bool>,
    cancel: Option<CancellationToken>,
) -> AppResult<String> {
    tracing::info!(
        language = language.as_deref().unwrap_or("auto"),
        diarize = diarize.unwrap_or(true),
        "Transcription requested"
    );

    // --- emit: loading ---
    let _ = app.emit("transcription-progress", "loading");

    // Parse the recording ID.
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    // Load the recording and mark as Processing — on a blocking thread.
    let db = Arc::clone(&state.db);
    let recording = tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        let mut recording = RecordingsRepo::get_by_id(&conn, &uuid)
            .map_err(|e| AppError::Database(e.to_string()))?;

        recording.status = ProcessingStatus::Processing {
            started_at: Utc::now(),
        };
        RecordingsRepo::update(&conn, &recording)
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok::<_, AppError>(recording)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    let wav_path = recording.audio_path.clone();

    if !wav_path.exists() {
        let err_msg = format!("WAV file not found: {}", wav_path.display());
        return Err(AppError::Processing(
            mark_recording_failed(&app, &state.db, recording, err_msg).await,
        ));
    }

    // Load and decode the WAV file on a blocking thread (CPU-intensive for large files).
    let wav_path_clone = wav_path.clone();
    let audio = match tokio::task::spawn_blocking(move || load_wav_to_audio_data(&wav_path_clone))
        .await
    {
        Ok(Ok(audio)) => audio,
        Ok(Err(e)) => {
            let err_msg = unwrap_app_error_message(e);
            return Err(AppError::Processing(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
        Err(e) => {
            let err_msg = format!("Task join error: {e}");
            return Err(AppError::Other(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
    };

    // Compute audio signal stats to detect silent/corrupt recordings.
    let (peak, rms) = if audio.samples.is_empty() {
        (0.0f32, 0.0f32)
    } else {
        let peak = audio.samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let sum_sq: f64 = audio.samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        let rms = (sum_sq / audio.samples.len() as f64).sqrt() as f32;
        (peak, rms)
    };

    tracing::info!(
        samples = audio.samples.len(),
        sample_rate = audio.sample_rate,
        channels = audio.channels,
        duration_secs = %format!("{:.1}", audio.duration_seconds()),
        peak_amplitude = %format!("{:.6}", peak),
        rms_level = %format!("{:.6}", rms),
        "Loaded WAV audio data"
    );

    // Detect near-silent recordings: RMS below -60 dBFS (~0.001) means the
    // microphone likely captured no speech.  Warn but proceed — Whisper's
    // empty-segment filter will catch it if there truly is no speech.
    if !audio.samples.is_empty() && rms < 0.001 {
        tracing::warn!(
            peak = %format!("{:.6}", peak),
            rms = %format!("{:.6}", rms),
            "Recording appears to be silent or near-silent — transcription may produce no text"
        );
    }

    if audio.samples.is_empty() {
        let err_msg = format!("WAV file contains no audio samples: {}", wav_path.display());
        tracing::error!("{err_msg}");
        return Err(AppError::Processing(
            mark_recording_failed(&app, &state.db, recording.clone(), err_msg).await,
        ));
    }

    // Build STT config from caller parameters.
    // Default diarize to true — medical recordings are typically conversations.
    let config = SttConfig {
        language,
        diarize: diarize.unwrap_or(true),
        ..SttConfig::default()
    };

    // Checkpoint: bail before the (potentially 30s+) STT call if cancelled.
    if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
        let err_msg = "Transcription cancelled before STT start".to_string();
        tracing::info!("{err_msg}");
        mark_recording_failed_db_only(&state.db, recording, err_msg).await;
        let _ = app.emit("transcription-progress", "failed");
        return Err(AppError::Cancelled);
    }

    // --- emit: transcribing ---
    let _ = app.emit("transcription-progress", "transcribing");

    let stt: Arc<dyn medical_core::traits::SttProvider + Send + Sync> = {
        let guard = state.stt_providers.lock().await;
        match guard.as_ref() {
            Some(stt) => stt.clone(),
            None => {
                let err_msg = "No STT provider configured. Download a Whisper model in Settings → Audio / STT.".to_string();
                tracing::error!("{err_msg}");
                return Err(AppError::SttProvider(
                    mark_recording_failed(&app, &state.db, recording, err_msg).await,
                ));
            }
        }
    };
    let token = cancel.clone().unwrap_or_default();
    let transcript = match stt.transcribe(audio, config, token).await {
        Ok(t) => t,
        Err(e) => {
            let err_msg = format!("Transcription failed: {e}");
            tracing::error!(error = %e, "STT transcription failed");
            return Err(AppError::Processing(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
    };

    tracing::info!(
        provider = %transcript.provider,
        text_len = transcript.text.len(),
        segments = transcript.segments.len(),
        "Transcription complete"
    );

    // Build speaker-attributed text when diarization segments are available.
    let display_text = format_transcript_with_speakers(&transcript);

    // Guard: silent source + repeated-phrase output is a Whisper hallucination.
    // Rejecting here stops us from generating a bogus SOAP from nonsense like
    // "Thank you. Thank you. Thank you. ..." that Whisper emits on silence.
    if rms < 0.001 && is_repeated_phrase_hallucination(&transcript.text) {
        let err_msg = format!(
            "Transcription rejected: the audio was effectively silent (rms={rms:.6}) and the model returned a repeated-phrase hallucination. Check your microphone or audio routing."
        );
        tracing::warn!(
            provider = %transcript.provider,
            rms = %format!("{:.6}", rms),
            text_preview = %transcript.text.chars().take(80).collect::<String>(),
            "Rejecting likely Whisper hallucination from silent source"
        );
        return Err(AppError::Processing(
            mark_recording_failed(&app, &state.db, recording, err_msg).await,
        ));
    }

    // Guard: if transcription produced no text, mark as Failed rather than
    // silently storing an empty transcript as "Completed".
    if display_text.trim().is_empty() {
        let err_msg = "Transcription produced no text — the recording may be silent or too short.".to_string();
        tracing::warn!(
            provider = %transcript.provider,
            segments = transcript.segments.len(),
            "{err_msg}"
        );
        return Err(AppError::Processing(
            mark_recording_failed(&app, &state.db, recording, err_msg).await,
        ));
    }

    // Checkpoint: bail before vocabulary correction if cancelled mid-STT.
    if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
        let err_msg = "Transcription cancelled after STT completion".to_string();
        tracing::info!("{err_msg}");
        mark_recording_failed_db_only(&state.db, recording, err_msg).await;
        let _ = app.emit("transcription-progress", "failed");
        return Err(AppError::Cancelled);
    }

    // Apply vocabulary corrections if enabled.
    //
    // When paired with an office server that exposes the vocab API, fetch
    // entries from there so corrections stay consistent across all paired
    // clients. On failure (server unreachable, transient HTTP error), warn
    // and fall through with no corrections rather than aborting the whole
    // transcription — corrections are best-effort polish on top of the
    // already-successful STT output.
    let vocab_enabled = {
        let db_settings = Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || -> bool {
            let conn = match db_settings.conn() {
                Ok(c) => c,
                Err(_) => return true,
            };
            SettingsRepo::load_config(&conn)
                .ok()
                .map(|mut c| { c.migrate(); c.vocabulary_enabled })
                .unwrap_or(true)
        })
        .await
        .unwrap_or(true)
    };

    let remote_entries: Option<Vec<medical_core::types::vocabulary::VocabularyEntry>> =
        if vocab_enabled {
            if let Some(conn) = crate::state::load_paired_connection() {
                if conn.ports.vocab.is_some() {
                    let bearer = crate::state::load_sharing_bearer();
                    if let Some(remote) = crate::vocab_remote::VocabRemote::from(&conn, bearer, state.http_client.clone()) {
                        match remote.list(None).await {
                            Ok(list) => {
                                Some(list.into_iter().filter(|e| e.enabled).collect())
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "remote vocab fetch failed; skipping corrections: {e}"
                                );
                                None
                            }
                        }
                    } else { None }
                } else { None }
            } else { None }
        } else { None };

    let db_vocab = Arc::clone(&state.db);
    let display_text = match tokio::task::spawn_blocking(move || {
        if !vocab_enabled {
            return Ok::<String, AppError>(display_text);
        }
        let entries = if let Some(remote) = remote_entries {
            remote
        } else {
            // Local fallback: only when not paired or remote fetch failed
            // and we want to use the local DB. When paired but remote
            // failed, remote_entries is None and we skip rather than
            // silently using stale local data.
            if crate::state::load_paired_connection().is_some() {
                return Ok(display_text);
            }
            let conn = db_vocab
                .conn()
                .map_err(|e| AppError::Database(e.to_string()))?;
            VocabularyRepo::list_enabled(&conn)
                .map_err(|e| AppError::Database(e.to_string()))?
        };
        if entries.is_empty() {
            return Ok(display_text);
        }
        let result = vocabulary_corrector::apply_corrections(&display_text, &entries);
        if result.total_replacements > 0 {
            tracing::info!(
                replacements = result.total_replacements,
                "Applied vocabulary corrections to transcript"
            );
        }
        Ok(result.corrected_text)
    })
    .await
    {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            let err_msg = unwrap_app_error_message(e);
            return Err(AppError::Processing(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
        Err(e) => {
            let err_msg = format!("Task join error: {e}");
            return Err(AppError::Other(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
    };

    // Persist the transcript and mark as Completed — on a blocking thread.
    // If persistence fails, route through mark_recording_failed so the frontend
    // spinner clears and the user sees a real error instead of a stuck
    // `Processing` state. The transcript text is logged via tracing::error! so
    // operators can recover it manually from logs.
    let mut recording = recording;
    recording.transcript = Some(display_text.clone());
    recording.stt_provider = Some(transcript.provider.clone());
    recording.status = ProcessingStatus::Completed {
        completed_at: Utc::now(),
    };

    let recording_for_failure = recording.clone();
    let join_result = tokio::task::spawn_blocking({
        let db = Arc::clone(&state.db);
        let recording_owned = recording;
        move || {
            let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
            RecordingsRepo::update(&conn, &recording_owned)
                .map_err(|e| AppError::Database(e.to_string()))?;
            Ok::<(), AppError>(())
        }
    })
    .await;

    match join_result {
        Ok(Ok(())) => { /* success — fall through to emit */ }
        Ok(Err(db_err)) => {
            let inner = unwrap_app_error_message(db_err);
            let err_msg = format!(
                "Transcription succeeded but failed to persist final status: {inner}"
            );
            let dir = match app.path().app_data_dir() {
                Ok(d) => d,
                Err(_) => std::env::temp_dir(),
            };
            match persist_orphaned_transcript(&dir, &recording_for_failure.id, &display_text) {
                Ok(path) => tracing::error!(
                    recording_id = %recording_for_failure.id,
                    orphaned_transcript_path = %path.display(),
                    "{err_msg} — orphaned transcript written for manual recovery"
                ),
                Err(io_err) => tracing::error!(
                    recording_id = %recording_for_failure.id,
                    orphan_persist_error = %io_err,
                    "{err_msg} — additionally failed to write orphaned transcript file"
                ),
            }
            return Err(AppError::Database(
                mark_recording_failed(&app, &state.db, recording_for_failure, err_msg).await,
            ));
        }
        Err(join_err) => {
            let err_msg = format!(
                "Transcription succeeded but DB persist task panicked: {join_err}"
            );
            let dir = match app.path().app_data_dir() {
                Ok(d) => d,
                Err(_) => std::env::temp_dir(),
            };
            match persist_orphaned_transcript(&dir, &recording_for_failure.id, &display_text) {
                Ok(path) => tracing::error!(
                    recording_id = %recording_for_failure.id,
                    orphaned_transcript_path = %path.display(),
                    "{err_msg} — orphaned transcript written for manual recovery"
                ),
                Err(io_err) => tracing::error!(
                    recording_id = %recording_for_failure.id,
                    orphan_persist_error = %io_err,
                    "{err_msg} — additionally failed to write orphaned transcript file"
                ),
            }
            return Err(AppError::Other(
                mark_recording_failed(&app, &state.db, recording_for_failure, err_msg).await,
            ));
        }
    }

    // --- emit: complete ---
    let _ = app.emit("transcription-progress", "complete");

    Ok(display_text)
}

/// Format a transcript with speaker labels when diarization data is available.
///
/// Groups consecutive segments by speaker and formats as:
///   Speaker 1: Hello, how are you?
///   Speaker 2: I'm not feeling well.
///
/// Falls back to the raw text when no speaker segments are present.
fn format_transcript_with_speakers(transcript: &medical_core::types::stt::Transcript) -> String {
    let segments_with_speakers: Vec<_> = transcript
        .segments
        .iter()
        .filter(|s| s.speaker.is_some())
        .collect();

    if segments_with_speakers.is_empty() {
        return transcript.text.clone();
    }

    // Group consecutive segments by speaker into paragraphs.
    // Speaker labels arrive pre-formatted from the merge module ("Speaker 1", "Speaker 2").
    let mut result = String::new();
    let mut current_speaker: Option<&str> = None;
    let mut current_words: Vec<&str> = Vec::new();

    for seg in &segments_with_speakers {
        let label = seg.speaker.as_deref().unwrap_or("Unknown");

        if current_speaker != Some(label) {
            // Flush the previous speaker's words.
            if !current_words.is_empty() {
                if let Some(prev) = current_speaker {
                    if !result.is_empty() {
                        result.push_str("\n\n");
                    }
                    result.push_str(prev);
                    result.push_str(": ");
                    result.push_str(&current_words.join(" "));
                }
                current_words.clear();
            }
            current_speaker = Some(label);
        }

        current_words.push(seg.text.trim());
    }

    // Flush the last speaker's words.
    if !current_words.is_empty()
        && let Some(prev) = current_speaker {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(prev);
            result.push_str(": ");
            result.push_str(&current_words.join(" "));
        }

    result
}
