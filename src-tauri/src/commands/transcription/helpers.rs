//! Audio loading, hallucination detection, orphaned-transcript persistence,
//! and failure-bookkeeping for [`super::inner`].

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::recording::ProcessingStatus;
use medical_core::types::stt::AudioData;
use medical_db::recordings::RecordingsRepo;
use tauri::Emitter;

/// Detect the repeated-short-phrase pattern Whisper produces when fed silence
/// (classic: "Thank you. Thank you. Thank you. ...").
///
/// Conservative by design: requires at least 3 sentence-like segments that are
/// all identical (case-insensitive, whitespace-normalised) and short. Callers
/// should gate this on a known-silent source so legitimate short transcripts
/// aren't rejected.
pub(super) fn is_repeated_phrase_hallucination(text: &str) -> bool {
    let segments: Vec<String> = text
        .split(['.', '!', '?', '\n'])
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 3 {
        return false;
    }
    let first = &segments[0];
    // Long segments are almost never hallucinations — real speech is varied.
    if first.chars().count() > 80 {
        return false;
    }
    segments.iter().all(|s| s == first)
}

/// Write an orphaned transcript (one whose DB persistence failed despite
/// successful transcription) to a file inside `app_data_dir/orphaned_transcripts/`.
/// Returns the full path so the caller can log it for manual recovery.
///
/// Lives inside the app data directory (same PHI boundary as the DB itself);
/// avoids putting raw transcript text into the global tracing pipeline.
pub(super) fn persist_orphaned_transcript(
    app_data_dir: &std::path::Path,
    recording_id: &uuid::Uuid,
    transcript: &str,
) -> std::io::Result<std::path::PathBuf> {
    let dir = app_data_dir.join("orphaned_transcripts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{recording_id}.txt"));
    std::fs::write(&path, transcript)?;
    Ok(path)
}

/// Compute the divisor used to normalize integer WAV samples to `[-1.0, 1.0]`.
///
/// Returns an error for `bits_per_sample == 0`, which would otherwise
/// trigger a shift-overflow panic in `1 << (bps - 1)` on debug builds and
/// a wrap-then-shift-overflow on release. Such a WAV is malformed; reject
/// it rather than crash the app.
fn compute_int_max_val(bits_per_sample: u16) -> AppResult<f32> {
    if bits_per_sample == 0 {
        return Err(AppError::Processing(
            "Corrupt WAV: bits_per_sample is 0".to_string(),
        ));
    }
    Ok((1u64 << (bits_per_sample - 1)) as f32)
}

/// Load a WAV file from disk and convert it into `AudioData` (f32 PCM).
pub(super) fn load_wav_to_audio_data(path: &std::path::Path) -> Result<AudioData, AppError> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| AppError::Processing(format!("Failed to open WAV: {e}")))?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader
                .into_samples::<f32>()
                .collect::<Result<Vec<f32>, _>>()
                .map_err(|e| AppError::Processing(format!("Corrupt WAV sample data: {e}")))?
        }
        hound::SampleFormat::Int => {
            let max_val = compute_int_max_val(spec.bits_per_sample)?;
            reader
                .into_samples::<i32>()
                .collect::<Result<Vec<i32>, _>>()
                .map_err(|e| AppError::Processing(format!("Corrupt WAV sample data: {e}")))?
                .into_iter()
                .map(|s| s as f32 / max_val)
                .collect()
        }
    };

    Ok(AudioData {
        samples,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    })
}

/// Persist `Failed` status for a recording. Ignores DB errors — the caller is
/// already returning the original error, so a DB write failure here would only
/// obscure it. This is the testable half of `mark_recording_failed`.
pub(super) async fn mark_recording_failed_db_only(
    db: &Arc<medical_db::Database>,
    mut recording: medical_core::types::recording::Recording,
    err_msg: String,
) {
    recording.status = ProcessingStatus::Failed {
        error: err_msg,
        retry_count: 0,
    };
    let db = Arc::clone(db);
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = db.conn() {
            let _ = RecordingsRepo::update(&conn, &recording);
        }
    })
    .await;
}

/// Mark a recording as `Failed`, persist the status, and emit
/// `transcription-progress: "failed"` so the frontend spinner clears.
///
/// Returns the error message unchanged so callers can
/// `return Err(mark_recording_failed(...).await);`.
pub(super) async fn mark_recording_failed(
    app: &tauri::AppHandle,
    db: &Arc<medical_db::Database>,
    recording: medical_core::types::recording::Recording,
    err_msg: String,
) -> String {
    mark_recording_failed_db_only(db, recording, err_msg.clone()).await;
    // Emit failure is non-fatal: at worst the frontend spinner stays visible until the next state change.
    let _ = app.emit("transcription-progress", "failed");
    err_msg
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;
    use medical_core::types::recording::{ProcessingStatus, Recording};
    use medical_db::Database;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn mk_recording() -> Recording {
        let mut rec = Recording::new("t.wav", PathBuf::from("/tmp/nope.wav"));
        rec.status = ProcessingStatus::Processing {
            started_at: Utc::now(),
        };
        rec
    }

    #[tokio::test]
    async fn mark_recording_failed_updates_status_to_failed() {
        let db = Arc::new(Database::open_in_memory().expect("open in-memory db"));
        let rec = mk_recording();
        let id = rec.id;
        {
            let conn = db.conn().expect("conn");
            RecordingsRepo::insert(&conn, &rec).expect("insert");
        }

        mark_recording_failed_db_only(&db, rec, "boom".to_string()).await;

        let conn = db.conn().expect("conn");
        let loaded = RecordingsRepo::get_by_id(&conn, &id).expect("get");
        match loaded.status {
            ProcessingStatus::Failed { error, retry_count } => {
                assert_eq!(error, "boom");
                assert_eq!(retry_count, 0);
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn unwrap_app_error_message_strips_prefix() {
        // AppError::Processing has a "Processing error: " display prefix — the helper
        // must return the raw inner string, not the Display output.
        let err = AppError::Processing("Failed to open WAV: foo".to_string());
        assert_eq!(
            crate::commands::unwrap_app_error_message(err),
            "Failed to open WAV: foo"
        );
    }

    #[test]
    fn detects_thank_you_hallucination() {
        assert!(is_repeated_phrase_hallucination(
            "Thank you. Thank you. Thank you. Thank you."
        ));
    }

    #[test]
    fn detects_case_insensitive_repetition() {
        assert!(is_repeated_phrase_hallucination(
            "thank you. Thank You. THANK YOU."
        ));
    }

    #[test]
    fn rejects_varied_speech() {
        assert!(!is_repeated_phrase_hallucination(
            "The patient reports fatigue. Blood pressure is 140 over 90. Continue current medications."
        ));
    }

    #[test]
    fn rejects_short_transcript() {
        assert!(!is_repeated_phrase_hallucination("Thank you."));
        assert!(!is_repeated_phrase_hallucination("Thank you. Thank you."));
    }

    #[test]
    fn rejects_empty_transcript() {
        assert!(!is_repeated_phrase_hallucination(""));
        assert!(!is_repeated_phrase_hallucination("   "));
    }

    #[test]
    fn rejects_long_repeated_segments() {
        // Long repeated segments are probably real speech, not hallucination.
        let long = "a".repeat(100);
        let text = format!("{long}. {long}. {long}.");
        assert!(!is_repeated_phrase_hallucination(&text));
    }

    #[test]
    fn compute_int_max_val_rejects_zero_bits() {
        let err = compute_int_max_val(0).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("bits_per_sample") || msg.contains("0"),
            "expected helpful error mentioning bits_per_sample or 0, got: {msg}"
        );
    }

    #[test]
    fn compute_int_max_val_handles_typical_widths() {
        // Common widths used by hound's Int sample format.
        assert_eq!(compute_int_max_val(8).unwrap(), 128.0);
        assert_eq!(compute_int_max_val(16).unwrap(), 32_768.0);
        assert_eq!(compute_int_max_val(24).unwrap(), 8_388_608.0);
        assert_eq!(compute_int_max_val(32).unwrap(), 2_147_483_648.0);
    }

    #[test]
    fn persist_orphaned_transcript_writes_file_in_subdir() {
        use std::fs;
        use uuid::Uuid;
        let tmp = tempfile::tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let transcript = "patient says hello";

        let path = persist_orphaned_transcript(tmp.path(), &id, transcript)
            .expect("persist");

        assert!(path.starts_with(tmp.path().join("orphaned_transcripts")));
        assert_eq!(path.file_name().unwrap().to_string_lossy(), format!("{id}.txt"));
        let on_disk = fs::read_to_string(&path).expect("read");
        assert_eq!(on_disk, transcript);
    }

    #[test]
    fn persist_orphaned_transcript_creates_subdir_if_missing() {
        use uuid::Uuid;
        let tmp = tempfile::tempdir().expect("tempdir");
        let id = Uuid::new_v4();

        // Subdir doesn't exist yet
        assert!(!tmp.path().join("orphaned_transcripts").exists());

        persist_orphaned_transcript(tmp.path(), &id, "x").expect("persist");

        assert!(tmp.path().join("orphaned_transcripts").is_dir());
    }
}
