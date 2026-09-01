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

/// Collapse whisper.cpp repetition loops within a transcript segment.
///
/// Whisper sometimes produces loops like "okay okay okay all right all right
/// all right all right" within a single segment — a decoding pathology, not
/// real speech. This function detects when the segment's tokens repeat a
/// short sub-sequence (1-4 words) 3+ times and collapses the repetition to
/// a single instance.
///
/// Conservative: only collapses when the *entire* segment is dominated by
/// the repeated pattern (the repeated block covers > 60% of the segment).
/// A real sentence that happens to repeat a word ("the patient said the
/// patient said...") won't be touched because the surrounding words break
/// the pattern.
pub(super) fn collapse_repetition_loops(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 6 {
        return text.to_string(); // too short to have a meaningful loop
    }

    // Try pattern lengths 1..=4 words.
    for pattern_len in 1..=4usize {
        if let Some(collapsed) = try_collapse_pattern(&tokens, pattern_len) {
            return collapsed;
        }
    }
    text.to_string()
}

/// If the tokens consist of a repeating `pattern_len`-word block (3+ times),
/// collapse to a single instance of that block.
fn try_collapse_pattern(tokens: &[&str], pattern_len: usize) -> Option<String> {
    if tokens.len() < pattern_len * 3 {
        return None;
    }
    let pattern: Vec<&str> = tokens[..pattern_len].to_vec();
    let pattern_lower: Vec<String> = pattern.iter().map(|t| t.to_lowercase()).collect();

    // Count how many consecutive repetitions of the pattern exist from the start.
    let mut reps = 1;
    let mut i = pattern_len;
    while i + pattern_len <= tokens.len() {
        let chunk_lower: Vec<String> = tokens[i..i + pattern_len]
            .iter()
            .map(|t| t.to_lowercase())
            .collect();
        if chunk_lower == pattern_lower {
            reps += 1;
            i += pattern_len;
        } else {
            break;
        }
    }

    if reps >= 3 && i == tokens.len() {
        // The entire segment is the pattern repeated — collapse to one.
        return Some(pattern.join(" "));
    }

    if reps >= 3 {
        // The repeated block covers the start; check if the remaining tokens
        // are short enough that the repetition dominates (> 60% of segment).
        let repeated_count = reps * pattern_len;
        if repeated_count as f64 / tokens.len() as f64 > 0.6 {
            let remaining = tokens[i..].join(" ");
            return Some(format!("{} {}", pattern.join(" "), remaining));
        }
    }

    None
}

/// Apply repetition-loop collapse to each segment of a transcript, returning
/// a new transcript text. Called after whisper transcription but before
/// formatting/storage.
pub(super) fn filter_segment_repetitions(transcript: &mut medical_core::types::stt::Transcript) {
    let mut changed = false;
    for seg in &mut transcript.segments {
        let original = &seg.text;
        let collapsed = collapse_repetition_loops(original);
        if collapsed != *original {
            tracing::warn!(
                original_len = original.len(),
                collapsed_len = collapsed.len(),
                "collapsed whisper repetition loop in segment"
            );
            seg.text = collapsed;
            changed = true;
        }
    }
    // Rebuild transcript.text if any segment was modified, so downstream
    // code (hallucination guard, formatting) sees consistent content.
    if changed {
        transcript.text = transcript
            .segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
    }
}

/// Drop runs of 3+ consecutive identical segments — whisper.cpp
/// repetition hallucinations. These occur when whisper's decoder gets
/// stuck in a high-probability loop during low-energy/noisy audio.
///
/// The original `MAX_WORDS = 5` limit was too restrictive — the real-world
/// patterns include long phrases ("I don't know if I was going to go
/// through it" = 10 words, "I see a counsellor once every two weeks"
/// = 9 words). Any segment text repeated 3+ times consecutively is a
/// hallucination, regardless of length.
///
/// After dropping, `transcript.text` is rebuilt from the surviving
/// segments so downstream code (hallucination guard, formatting) sees
/// accurate content.
pub(super) fn filter_cross_segment_repetitions(
    transcript: &mut medical_core::types::stt::Transcript,
) {
    const MIN_RUN_LEN: usize = 3;

    let segments = &transcript.segments;
    if segments.len() < MIN_RUN_LEN {
        return;
    }

    // Identify runs of consecutive identical segments.
    let mut drop_indices: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < segments.len() {
        let current_key = segments[i].text.trim().to_lowercase();
        if current_key.is_empty() {
            i += 1;
            continue;
        }
        // Count how many consecutive segments match (case-insensitive, trimmed).
        let run_end = i
            + 1
            + segments[i + 1..]
                .iter()
                .take_while(|s| s.text.trim().to_lowercase() == current_key)
                .count();
        let run_len = run_end - i;
        if run_len >= MIN_RUN_LEN {
            // PHI guard: log the length only — segment text is transcript
            // content and must never reach the persistent log.
            tracing::warn!(
                text_len = current_key.len(),
                run_len,
                "dropping cross-segment repetition (whisper hallucination)"
            );
            drop_indices.extend(i..run_end);
        }
        i = run_end;
    }

    if drop_indices.is_empty() {
        return;
    }

    // Retain only segments not in the drop set.
    let drop_set: std::collections::HashSet<usize> = drop_indices.into_iter().collect();
    transcript.segments = transcript
        .segments
        .iter()
        .enumerate()
        .filter(|(idx, _)| !drop_set.contains(idx))
        .map(|(_, seg)| seg.clone())
        .collect();

    // Rebuild the joined text so downstream guards see the filtered result.
    transcript.text = transcript
        .segments
        .iter()
        .map(|s| s.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
}

/// Open a recording WAV file and return the raw decrypted bytes.
///
/// Used by `export_audio` to get the full WAV (header + data) for
/// re-encoding as standard 16-bit PCM. Handles both encrypted (FE1)
/// and legacy plaintext files.
///
/// Reads the file **once** into memory, then branches on the in-memory
/// bytes. This avoids a TOCTOU race where the background encryption
/// task's atomic rename could land between `decrypt_file`'s read and
/// the `NotEncrypted` fallback's second `std::fs::read`.
pub(crate) fn open_recording_wav_raw(path: &std::path::Path) -> AppResult<Vec<u8>> {
    use medical_security::file_crypto::{FileCryptoError, decrypt_bytes};

    let bytes = std::fs::read(path).map_err(AppError::from)?;
    match decrypt_bytes(&bytes) {
        Ok(plaintext) => Ok(plaintext),
        Err(FileCryptoError::NotEncrypted) => Ok(bytes), // legacy plaintext — use the bytes we already read
        Err(e) => Err(AppError::processing(format!(
            "Failed to decrypt recording: {e}"
        ))),
    }
}

/// Write an orphaned transcript (one whose DB persistence failed despite
/// successful transcription) to an **encrypted** file inside
/// `app_data_dir/orphaned_transcripts/`. Returns the full path so the
/// caller can log it for manual recovery.
///
/// Lives inside the app data directory (same PHI boundary as the DB itself);
/// avoids putting raw transcript text into the global tracing pipeline.
/// Encrypted with the same AES-256-GCM file key as audio recordings so the
/// plaintext never sits on disk. If encryption fails (keychain unavailable),
/// falls back to plaintext rather than losing the transcript entirely — the
/// caller logs the path either way.
pub(super) fn persist_orphaned_transcript(
    app_data_dir: &std::path::Path,
    recording_id: &uuid::Uuid,
    transcript: &str,
) -> std::io::Result<std::path::PathBuf> {
    let dir = app_data_dir.join("orphaned_transcripts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{recording_id}.enc"));
    // Best-effort encryption: the transcript is a recovery artifact and
    // must not be lost, so fall back to plaintext if the keychain is down.
    match medical_security::file_crypto::encrypt_file(&path, transcript.as_bytes()) {
        Ok(()) => {}
        Err(medical_security::file_crypto::FileCryptoError::Keychain(_)) => {
            // No keychain — write plaintext .txt so the data isn't lost.
            let plain = dir.join(format!("{recording_id}.txt"));
            std::fs::write(&plain, transcript)?;
            return Ok(plain);
        }
        Err(e) => {
            return Err(std::io::Error::other(format!(
                "encrypt orphaned transcript: {e}"
            )));
        }
    }
    Ok(path)
}

/// Compute the divisor used to normalize integer WAV samples to `[-1.0, 1.0]`.
///
/// Returns an error for `bits_per_sample == 0` or values > 32, which would
/// trigger a shift-overflow panic in `1 << (bps - 1)` on debug builds and
/// undefined behavior in release. Such a WAV is malformed; reject it rather
/// than crash the app.
fn compute_int_max_val(bits_per_sample: u16) -> AppResult<f32> {
    if bits_per_sample == 0 || bits_per_sample > 32 {
        return Err(AppError::processing(format!(
            "Corrupt WAV: bits_per_sample is {bits_per_sample} (must be 1-32)"
        )));
    }
    Ok((1u64 << (bits_per_sample - 1)) as f32)
}

/// Open a recording WAV file for reading, transparently decrypting it if
/// it's encrypted at rest.
///
/// Shared by `load_wav_to_audio_data` (transcription) and
/// `compute_audio_levels` (audio-level check). Returns a `WavReader` backed
/// by an in-memory buffer.
///
/// Reads the file **once** into memory, then branches on the in-memory
/// bytes via `decrypt_bytes`. This eliminates a TOCTOU race where the
/// background encryption task's atomic rename could land between
/// `decrypt_file`'s internal read and the `NotEncrypted` fallback's
/// second `std::fs::read` — which would cause hound to see encrypted
/// bytes (FE1 magic) instead of the RIFF header it expects.
pub(crate) fn open_recording_wav(
    path: &std::path::Path,
) -> Result<hound::WavReader<std::io::Cursor<Vec<u8>>>, AppError> {
    use medical_security::file_crypto::{FileCryptoError, decrypt_bytes};

    let raw_bytes = std::fs::read(path)
        .map_err(|e| AppError::processing(format!("Failed to read WAV file: {e}")))?;

    let wav_bytes: Vec<u8> = match decrypt_bytes(&raw_bytes) {
        Ok(plaintext) => plaintext,
        Err(FileCryptoError::NotEncrypted) => raw_bytes, // legacy plaintext — use the bytes we already read
        Err(e) => {
            return Err(AppError::processing(format!(
                "Failed to decrypt recording: {e}"
            )));
        }
    };

    hound::WavReader::new(std::io::Cursor::new(wav_bytes))
        .map_err(|e| AppError::processing(format!("Failed to open WAV: {e}")))
}

/// Load a WAV file from disk and convert it into `AudioData` (f32 PCM).
///
/// Handles both encrypted recordings (the default since at-rest encryption
/// shipped) and legacy plaintext WAVs — `file_crypto::decrypt_file` returns
/// `NotEncrypted` for the latter, in which case we read the file directly.
pub(super) fn load_wav_to_audio_data(path: &std::path::Path) -> Result<AudioData, AppError> {
    let reader = open_recording_wav(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<f32>, _>>()
            .map_err(|e| AppError::processing(format!("Corrupt WAV sample data: {e}")))?,
        hound::SampleFormat::Int => {
            let max_val = compute_int_max_val(spec.bits_per_sample)?;
            reader
                .into_samples::<i32>()
                .collect::<Result<Vec<i32>, _>>()
                .map_err(|e| AppError::processing(format!("Corrupt WAV sample data: {e}")))?
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
        if let Ok(conn) = db.conn()
            && let Err(e) = RecordingsRepo::update(&conn, &recording)
        {
            // The failure-marker write itself failed. The recording will stay
            // stuck in `Processing` in the DB; surface this in the log so it's
            // debuggable instead of silently dropping the error.
            tracing::warn!(
                error = %e,
                rec = %recording.id,
                "failed to mark recording Failed in DB"
            );
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
        let err = AppError::processing("Failed to open WAV: foo".to_string());
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

        let path = persist_orphaned_transcript(tmp.path(), &id, transcript).expect("persist");

        assert!(path.starts_with(tmp.path().join("orphaned_transcripts")));
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        // The file is either encrypted (.enc) or plaintext (.txt fallback
        // when the keychain is unavailable in this environment).
        if let Some(stem) = fname.strip_suffix(".enc") {
            assert_eq!(stem, id.to_string());
            // Verify it's actually encrypted (magic prefix) and decrypts
            // back to the original transcript.
            let bytes = fs::read(&path).expect("read");
            assert!(bytes.starts_with(medical_security::file_crypto::MAGIC));
            let decrypted =
                medical_security::file_crypto::decrypt_bytes(&bytes).expect("decryption roundtrip");
            assert_eq!(String::from_utf8(decrypted).unwrap(), transcript);
        } else if let Some(stem) = fname.strip_suffix(".txt") {
            // Plaintext fallback (no keychain in this test env).
            assert_eq!(stem, id.to_string());
            assert_eq!(fs::read_to_string(&path).expect("read"), transcript);
        } else {
            panic!("unexpected filename: {fname}");
        }
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

    // ---- filter_cross_segment_repetitions tests ----

    use medical_core::types::stt::{Transcript, TranscriptSegment};

    fn seg(text: &str, start: f64) -> TranscriptSegment {
        TranscriptSegment {
            text: text.into(),
            start,
            end: start + 5.0,
            speaker: None,
            confidence: None,
        }
    }

    fn mk_transcript(segments: Vec<TranscriptSegment>) -> Transcript {
        let text = segments
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        Transcript {
            text,
            segments,
            language: None,
            duration_seconds: None,
            provider: "test".into(),
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn filter_cross_segment_repetitions_strips_trailing_so_so_so() {
        // The exact user-reported bug: real conversation followed by 6× "so".
        let mut t = mk_transcript(vec![
            seg("you need some medication refills", 0.0),
            seg("which medications okay", 5.0),
            seg("you're still using that", 10.0),
            seg("so", 55.0),
            seg("so", 65.0),
            seg("so", 75.0),
            seg("so", 85.0),
            seg("so", 95.0),
            seg("so", 115.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 3, "should keep only the 3 real segments");
        assert!(
            !t.text.contains(" so "),
            "rebuilt text should not contain the dropped 'so'"
        );
        assert!(t.text.contains("medication"));
    }

    #[test]
    fn filter_cross_segment_repetitions_keeps_distinct_segments() {
        let mut t = mk_transcript(vec![
            seg("Patient reports headache", 0.0),
            seg("BP 120 over 80", 5.0),
            seg("Tension headache likely", 10.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 3, "distinct segments unchanged");
    }

    #[test]
    fn filter_cross_segment_repetitions_keeps_short_runs() {
        // 2 identical segments is below the 3+ threshold.
        let mut t = mk_transcript(vec![
            seg("okay", 0.0),
            seg("okay", 5.0),
            seg("next topic", 10.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 3, "runs < 3 are kept");
    }

    #[test]
    fn filter_cross_segment_repetitions_drops_long_repeated_phrases() {
        // The user-reported bug: a 10-word phrase repeated 3+ times.
        // The old MAX_WORDS=5 limit missed these; now any identical
        // segment repeated 3+ times is dropped.
        let phrase = "I don't know if I was going to go through it";
        let mut t = mk_transcript(vec![
            seg("Patient came in today", 0.0),
            seg(phrase, 10.0),
            seg(phrase, 20.0),
            seg(phrase, 30.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(
            t.segments.len(),
            1,
            "repeated long phrases should be dropped"
        );
        assert!(t.text.contains("Patient came in today"));
    }
}
