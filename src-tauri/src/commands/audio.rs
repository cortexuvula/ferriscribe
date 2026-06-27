use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tauri::Emitter;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use medical_audio::capture::CaptureConfig;
use medical_audio::device::{AudioDevice, get_input_device, list_input_devices};
use medical_core::error::{AppError, AppResult};
use medical_core::types::recording::{ProcessingStatus, Recording};
use medical_db::recordings::RecordingsRepo;

use super::resolve_recordings_dir;
use crate::state::{AppState, CurrentRecording, SendCaptureHandle};

// ──────────────────────────────────────────────────────────────────────────────
// 1. list_audio_devices
// ──────────────────────────────────────────────────────────────────────────────

/// List available audio input devices.
///
/// Returns device metadata (name, channels, sample rate) for the audio
/// device picker in the settings UI.
#[tauri::command]
pub fn list_audio_devices() -> AppResult<Vec<AudioDevice>> {
    list_input_devices().map_err(|e| AppError::Audio(e.to_string()))
}

// ──────────────────────────────────────────────────────────────────────────────
// 2. start_recording
// ──────────────────────────────────────────────────────────────────────────────

/// Start an audio recording session.
///
/// Opens the configured audio device, creates a WAV file in the recordings
/// directory, and begins capturing. Returns the new recording's UUID.
/// Fails if a recording is already in progress.
#[tauri::command]
#[instrument(skip(app, state), name = "audio::start_recording")]
pub async fn start_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<String> {
    info!("Starting audio recording");

    // Atomically check-and-set recording flag to prevent concurrent recordings.
    {
        let mut active = state.recording_active.lock().await;
        if *active {
            warn!("Attempted to start recording while another is in progress");
            return Err(AppError::Audio(
                "A recording is already in progress".to_string(),
            ));
        }
        *active = true;
    }

    // Helper: reset recording_active on error so the user isn't locked out.
    macro_rules! try_or_reset {
        ($state:expr, $expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    let mut active = $state.recording_active.lock().await;
                    *active = false;
                    return Err(e);
                }
            }
        };
    }

    // Resolve recordings directory from settings (custom path or default).
    let recordings_dir = try_or_reset!(state, resolve_recordings_dir(&state.db, &state.data_dir));

    // Generate UUID and human-readable filename.
    let recording_id = Uuid::new_v4();
    let now = chrono::Local::now();
    let friendly_name = now.format("Recording_%Y-%m-%d_%H-%M-%S").to_string();
    let wav_path = recordings_dir.join(format!("{}.wav", friendly_name));

    // Read the configured input device and sample rate from settings.
    let (input_device_name, sample_rate) = try_or_reset!(state, {
        let conn = state
            .db
            .conn()
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut config = medical_db::settings::SettingsRepo::load_config(&conn)
            .map_err(|e| AppError::Database(e.to_string()))?;
        config.migrate();
        Ok::<_, AppError>((
            config.input_device.filter(|s| !s.is_empty()),
            config.sample_rate,
        ))
    });

    // Capture values for logging before they move into closures.
    let device_name_for_log = input_device_name
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let wav_path_for_log = wav_path.display().to_string();

    // Start capture on a dedicated std::thread so the !Send CaptureHandle
    // never crosses a thread boundary via tokio::spawn_blocking.  We wrap
    // it in SendCaptureHandle (which has an unsafe Send impl) and send it
    // back through a oneshot channel.
    let wav_path_clone = wav_path.clone();
    let (tx, rx) = std::sync::mpsc::channel::<
        Result<(SendCaptureHandle, std::sync::mpsc::Receiver<Vec<f32>>), AppError>,
    >();

    std::thread::spawn(move || {
        let result = (|| {
            let device = get_input_device(input_device_name.as_deref())
                .map_err(|e| AppError::Audio(e.to_string()))?;
            let config = CaptureConfig {
                sample_rate,
                ..CaptureConfig::default()
            };
            let (handle, waveform_rx) =
                medical_audio::capture::start_capture(&device, config, &wav_path_clone)
                    .map_err(|e| AppError::Audio(e.to_string()))?;
            Ok((SendCaptureHandle(Some(handle)), waveform_rx))
        })();
        let _ = tx.send(result);
    });

    let (send_handle, waveform_rx) = try_or_reset!(
        state,
        rx.recv()
            .map_err(|_| AppError::Audio("Audio capture thread panicked".to_string()))
            .and_then(|r| r)
    );

    // Store capture handle in AppState.
    {
        let mut handle_lock = state
            .capture_handle
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
        *handle_lock = send_handle;
    }

    // Store current recording info.
    {
        let mut rec_lock = state
            .current_recording
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
        *rec_lock = Some(CurrentRecording {
            id: recording_id.to_string(),
            wav_path,
            started_at: Instant::now(),
        });
    }

    info!(
        recording_id = %recording_id,
        wav_path = %wav_path_for_log,
        device = %device_name_for_log,
        sample_rate,
        "Audio recording started"
    );

    // Spawn a blocking task to consume waveform data and emit Tauri events.
    //
    // Lifecycle: this task exits when `waveform_rx.recv()` returns `Err`, which
    // happens when every `waveform_tx` sender has been dropped. The only
    // sender lives inside the capture drain thread (see `crates/audio/src/
    // capture.rs`); that thread exits when the `CaptureHandle` is dropped —
    // which `stop_recording` / `cancel_recording` do synchronously before
    // returning. So by the time the next `start_recording` could run, the
    // previous emit task has already unwound. No JoinHandle needed, but the
    // invariant is load-bearing: if you change the sender ownership in
    // capture.rs, revisit this site.
    let app_clone = app.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(data) = waveform_rx.recv() {
            let _ = app_clone.emit("waveform-data", &data);
        }
    });

    Ok(recording_id.to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// 3. stop_recording
// ──────────────────────────────────────────────────────────────────────────────

/// Stop the active recording and finalize the WAV file.
///
/// Drains the audio buffer, closes the capture stream, and updates the
/// recording's DB row with the final file size and duration. Returns the
/// recording ID.
#[tauri::command]
#[instrument(skip(state), name = "audio::stop_recording")]
pub async fn stop_recording(state: tauri::State<'_, AppState>) -> AppResult<String> {
    // Take the CaptureHandle out of AppState as a SendCaptureHandle (which is
    // Send+Sync).  We must NOT hold a bare CaptureHandle across an .await
    // because CaptureHandle is !Send.
    let wrapper = {
        let mut handle_lock = state
            .capture_handle
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
        let inner = handle_lock.0.take();
        SendCaptureHandle(inner)
    };

    if wrapper.0.is_none() {
        // Desync safety: clear the active flag too, otherwise a stale `true`
        // leaves the user permanently locked out of starting a new recording.
        {
            let mut active = state.recording_active.lock().await;
            *active = false;
        }
        return Err(AppError::Audio("No active recording to stop".to_string()));
    }

    // Drop the wrapper on a blocking worker so CaptureHandle::drop (which
    // joins the drain thread) doesn't block the async runtime.
    tokio::task::spawn_blocking(move || drop(wrapper))
        .await
        .map_err(|e| AppError::Other(format!("Stop task panicked: {e}")))?;

    // Set recording inactive.
    {
        let mut active = state.recording_active.lock().await;
        *active = false;
    }

    // Take the current recording info.
    let current = {
        let mut rec_lock = state
            .current_recording
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
        rec_lock.take()
    };

    let current =
        current.ok_or_else(|| AppError::Audio("No current recording info found".to_string()))?;

    // Compute duration from elapsed time.
    let duration_secs = current.started_at.elapsed().as_secs_f64();

    // Get file size of the WAV file.
    let file_size = match std::fs::metadata(&current.wav_path) {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::warn!(path = %current.wav_path.display(), error = %e, "Could not read WAV file metadata");
            0
        }
    };
    if file_size == 0 {
        tracing::warn!(path = %current.wav_path.display(), "WAV file is empty — audio may not have been captured");
    }

    // Encrypt the recording at rest. Audio is the most sensitive artifact
    // (patient voice); the DB is already SQLCipher-encrypted, but WAVs sat
    // plaintext on disk. Best-effort: if the keychain is unavailable, keep
    // the plaintext file (recording the encounter matters more) and warn.
    // encrypt_file_in_place is atomic (temp+rename) so a failure leaves the
    // original WAV intact.
    if file_size > 0 {
        match medical_security::file_crypto::encrypt_file_in_place(&current.wav_path) {
            Ok(()) => {
                tracing::debug!(path = %current.wav_path.display(), "Recording encrypted at rest");
            }
            Err(medical_security::file_crypto::FileCryptoError::Keychain(e)) => {
                tracing::warn!(error = %e, "Could not encrypt recording (keychain unavailable); storing plaintext");
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %current.wav_path.display(), "Could not encrypt recording; storing plaintext");
            }
        }
    }

    let recording_uuid = Uuid::parse_str(&current.id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    // Build the Recording struct.
    let filename = current
        .wav_path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.wav", current.id));

    let mut recording = Recording::new(filename, current.wav_path.clone());
    // Override the auto-generated id with our known UUID.
    recording.id = recording_uuid;
    recording.duration_seconds = Some(duration_secs);
    recording.file_size_bytes = Some(file_size);
    recording.status = ProcessingStatus::Pending;

    // Insert into DB.
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    RecordingsRepo::insert(&conn, &recording).map_err(|e| AppError::Database(e.to_string()))?;

    info!(
        recording_id = %current.id,
        duration_secs = %format!("{:.1}", duration_secs),
        file_size_bytes = file_size,
        wav_path = %current.wav_path.display(),
        "Recording stopped and saved"
    );

    Ok(current.id)
}

// ──────────────────────────────────────────────────────────────────────────────
// 4. cancel_recording
// ──────────────────────────────────────────────────────────────────────────────

/// Cancel the current recording, discarding the audio file without saving.
#[tauri::command]
pub async fn cancel_recording(state: tauri::State<'_, AppState>) -> AppResult<()> {
    // Take the CaptureHandle out of AppState.
    let wrapper = {
        let mut handle_lock = state
            .capture_handle
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
        let inner = handle_lock.0.take();
        SendCaptureHandle(inner)
    };

    if wrapper.0.is_none() {
        // Clear the active flag even on the no-op path so a desynced `true`
        // doesn't leave the user locked out.
        {
            let mut active = state.recording_active.lock().await;
            *active = false;
        }
        // Also clear any stale current_recording slot.
        {
            let mut rec_lock = state
                .current_recording
                .lock()
                .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
            *rec_lock = None;
        }
        return Err(AppError::Audio("No active recording to cancel".to_string()));
    }

    // Drop the capture handle on a blocking worker so its drop (which joins
    // the drain thread) doesn't stall the async runtime.
    tokio::task::spawn_blocking(move || drop(wrapper))
        .await
        .map_err(|e| AppError::Other(format!("Cancel task panicked: {e}")))?;

    // Set recording inactive.
    {
        let mut active = state.recording_active.lock().await;
        *active = false;
    }

    // Take the current recording info and delete the WAV file.
    let current = {
        let mut rec_lock = state
            .current_recording
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
        rec_lock.take()
    };

    if let Some(current) = current
        && current.wav_path.exists()
    {
        let _ = std::fs::remove_file(&current.wav_path);
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// 5. pause_recording
// ──────────────────────────────────────────────────────────────────────────────

/// Pause the active recording (stop writing audio samples to disk).
///
/// The capture stream remains open but audio data is discarded until
/// `resume_recording` is called.
#[tauri::command]
pub fn pause_recording(state: tauri::State<'_, AppState>) -> AppResult<()> {
    let handle_lock = state
        .capture_handle
        .lock()
        .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
    match &handle_lock.0 {
        Some(handle) => {
            handle.pause();
            Ok(())
        }
        None => Err(AppError::Audio("No active recording to pause".to_string())),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 6. resume_recording
// ──────────────────────────────────────────────────────────────────────────────

/// Resume a previously paused recording.
#[tauri::command]
pub fn resume_recording(state: tauri::State<'_, AppState>) -> AppResult<()> {
    let handle_lock = state
        .capture_handle
        .lock()
        .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
    match &handle_lock.0 {
        Some(handle) => {
            handle.resume();
            Ok(())
        }
        None => Err(AppError::Audio("No active recording to resume".to_string())),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 7. get_recording_state
// ──────────────────────────────────────────────────────────────────────────────

/// Snapshot of the current recording status, used by the frontend on boot to
/// recover from a webview reload that left an orphan capture running.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingStateSnapshot {
    pub active: bool,
    pub recording_id: Option<String>,
    pub elapsed_secs: Option<f64>,
}

/// Get the current recording state for the frontend's boot recovery.
///
/// Returns whether a recording is active, its ID, and elapsed time so the
/// frontend can recover from a webview reload that left an orphan capture
/// running.
#[tauri::command]
pub async fn get_recording_state(
    state: tauri::State<'_, AppState>,
) -> AppResult<RecordingStateSnapshot> {
    let active = *state.recording_active.lock().await;
    let current = {
        let guard = state
            .current_recording
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
        guard.as_ref().map(|c| (c.id.clone(), c.started_at))
    };
    let (recording_id, elapsed_secs) = match current {
        Some((id, started_at)) => (Some(id), Some(started_at.elapsed().as_secs_f64())),
        None => (None, None),
    };
    Ok(RecordingStateSnapshot {
        active,
        recording_id,
        elapsed_secs,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// 8. check_recording_audio_levels
// ──────────────────────────────────────────────────────────────────────────────

/// Stats reported by `check_recording_audio_levels`.
///
/// `peak` is the maximum absolute sample value (0.0–1.0 for float PCM).
/// `rms` is the root-mean-square level across all samples.
/// `is_silent` is true when rms < 0.001 (about -60 dBFS) — a threshold at which
/// Whisper tends to hallucinate rather than transcribe real content.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingAudioLevels {
    pub peak: f32,
    pub rms: f32,
    pub is_silent: bool,
}

/// Analyze the audio levels of a finished recording.
///
/// Returns peak, RMS, and a silence flag. Used by the frontend to warn when
/// a recording is too quiet (Whisper tends to hallucinate on silent audio).
#[tauri::command]
pub async fn check_recording_audio_levels(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<RecordingAudioLevels> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    let db = Arc::clone(&state.db);
    let recording = tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    let wav_path = recording.audio_path.clone();
    let levels = tokio::task::spawn_blocking(move || compute_audio_levels(&wav_path))
        .await
        .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    if levels.is_silent {
        warn!(
            recording_id = %recording_id,
            peak = %format!("{:.6}", levels.peak),
            rms = %format!("{:.6}", levels.rms),
            "Recording flagged as silent by check_recording_audio_levels"
        );
    }
    Ok(levels)
}

fn compute_audio_levels(path: &std::path::Path) -> AppResult<RecordingAudioLevels> {
    // Decrypt-then-open: handles encrypted recordings AND legacy plaintext.
    let reader = crate::commands::transcription::helpers::open_recording_wav(path)?;
    let spec = reader.spec();

    // Guard against malformed WAVs where bits_per_sample is 0. Without this
    // check, `1u64 << (spec.bits_per_sample - 1)` underflows to `1 << u32::MAX`
    // for the int branch, which is an undefined shift and yields garbage peak
    // and rms values. Return zeroed levels with a warning instead.
    if spec.bits_per_sample == 0 {
        tracing::warn!(
            path = %path.display(),
            sample_rate = spec.sample_rate,
            channels = spec.channels,
            "WAV header reports bits_per_sample=0; returning zeroed levels instead of computing bogus values"
        );
        return Ok(RecordingAudioLevels {
            peak: 0.0,
            rms: 0.0,
            is_silent: true,
        });
    }

    let (peak, sum_sq, count) = match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut peak = 0.0f32;
            let mut sum_sq = 0.0f64;
            let mut count: u64 = 0;
            for sample in reader.into_samples::<f32>() {
                let s =
                    sample.map_err(|e| AppError::Processing(format!("Corrupt WAV sample: {e}")))?;
                let abs = s.abs();
                if abs > peak {
                    peak = abs;
                }
                sum_sq += (s as f64) * (s as f64);
                count += 1;
            }
            (peak, sum_sq, count)
        }
        hound::SampleFormat::Int => {
            let max_val = (1u64 << (spec.bits_per_sample - 1)) as f32;
            let mut peak = 0.0f32;
            let mut sum_sq = 0.0f64;
            let mut count: u64 = 0;
            for sample in reader.into_samples::<i32>() {
                let raw =
                    sample.map_err(|e| AppError::Processing(format!("Corrupt WAV sample: {e}")))?;
                let s = raw as f32 / max_val;
                let abs = s.abs();
                if abs > peak {
                    peak = abs;
                }
                sum_sq += (s as f64) * (s as f64);
                count += 1;
            }
            (peak, sum_sq, count)
        }
    };

    let rms = if count == 0 {
        0.0f32
    } else {
        (sum_sq / count as f64).sqrt() as f32
    };

    Ok(RecordingAudioLevels {
        peak,
        rms,
        is_silent: count > 0 && rms < 0.001,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates a poisoned `std::sync::Mutex` and verifies the lock attempt
    /// produces an `AppError::MutexPoisoned` rather than panicking.
    #[test]
    fn poisoned_mutex_propagates_as_app_error() {
        use std::sync::{Arc, Mutex};

        let mutex: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let mutex_clone = Arc::clone(&mutex);

        // Poison the mutex by panicking while holding the lock.
        let handle = std::thread::spawn(move || {
            let _guard = mutex_clone.lock().unwrap();
            panic!("poison the lock");
        });
        // Swallow the panic from the spawned thread.
        let _ = handle.join();

        // Attempt to lock the now-poisoned mutex using the same pattern as
        // the production code.
        let result: AppResult<u32> = (|| {
            let guard = mutex
                .lock()
                .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
            Ok(*guard)
        })();

        match result {
            Err(AppError::MutexPoisoned(msg)) => {
                assert!(
                    msg.contains("capture_handle"),
                    "error message should include the lock name, got: {msg}"
                );
            }
            other => panic!(
                "expected Err(AppError::MutexPoisoned), got: {:?}",
                other.map(|_| "Ok")
            ),
        }
    }
}
