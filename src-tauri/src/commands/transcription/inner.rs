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

use crate::commands::resolve_recordings_dir;
use crate::commands::unwrap_app_error_message;
use crate::state::AppState;

use super::helpers::{
    filter_cross_segment_repetitions, filter_segment_repetitions, is_repeated_phrase_hallucination,
    load_wav_to_audio_data, mark_recording_failed, mark_recording_failed_db_only,
    persist_orphaned_transcript,
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
        diarize = diarize.unwrap_or(false),
        "Transcription requested"
    );

    // --- emit: loading ---
    let _ = app.emit("transcription-progress", "loading");

    // Parse the recording ID.
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    // Step 1: Load AppConfig (needed for pre-flight; no recording mutation yet).
    let app_config = crate::commands::load_app_config(&state.db, "preflight").await?;

    // Step 2: Pre-flight — probe the remote STT endpoint before mutating the
    // recording's status. For local whisper users stt_remote_host is empty so
    // this is a no-op. Placed BEFORE the Processing write so a failure leaves
    // the recording in its original (Pending) state instead of stuck in
    // Processing forever.
    medical_core::preflight::preflight_for_command(
        medical_core::preflight::CommandKind::Transcribe,
        &app_config,
    )
    .await?;

    // Step 3: Load the recording and mark as Processing — on a blocking thread.
    // Pre-flight passed, so it is now safe to advance the recording's status.
    let db = Arc::clone(&state.db);
    let mut recording = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        let mut recording = RecordingsRepo::get_by_id(&conn, &uuid)?;

        recording.status = ProcessingStatus::Processing {
            started_at: Utc::now(),
        };
        RecordingsRepo::update(&conn, &recording)?;
        // Field-revision stamp for the syncable status column (2026-08-17
        // review item 8) — the whole-row update above leaves the revision
        // table stale; the wire builder's max(revision,row) rider covers
        // LWW, this records the precise per-field clock. Best-effort: a
        // failed stamp must not fail the transcription over sync metadata.
        if let Err(e) = medical_db::ContentSyncRepo::upsert_revision(
            &conn,
            &uuid,
            "processing_status",
            &Utc::now().to_rfc3339(),
            None,
        ) {
            tracing::warn!(error = %e, "transcription: status revision stamp failed (non-fatal)");
        }
        Ok::<_, AppError>(recording)
    })
    .await
    .map_err(crate::commands::join_err)??;

    let wav_path = if recording.audio_path.exists() {
        recording.audio_path.clone()
    } else {
        // The stored path may be stale (user changed storage_path setting or
        // moved files). Try the current recordings directory by filename.
        if let Ok(dir) = resolve_recordings_dir(&state.db, &state.data_dir) {
            if let Some(filename) = recording.audio_path.file_name() {
                let candidate = dir.join(filename);
                if candidate.exists() {
                    tracing::info!(
                        original = %recording.audio_path.display(),
                        resolved = %candidate.display(),
                        "Resolved stale audio_path to current recordings directory"
                    );
                    // Persist the corrected path so future retries work directly.
                    recording.audio_path = candidate.clone();
                    // Wrap the SQLite update in spawn_blocking so we never
                    // block the async runtime worker.
                    let rec_clone = recording.clone();
                    let db = Arc::clone(&state.db);
                    tokio::task::spawn_blocking(move || -> AppResult<()> {
                        let conn = db.conn()?;
                        RecordingsRepo::update(&conn, &rec_clone)?;
                        Ok(())
                    })
                    .await
                    .map_err(crate::commands::join_err)??;
                    candidate
                } else {
                    let err_msg = format!("WAV file not found: {}", recording.audio_path.display());
                    return Err(AppError::processing(
                        mark_recording_failed(&app, &state.db, recording, err_msg).await,
                    ));
                }
            } else {
                let err_msg = format!("WAV file not found: {}", recording.audio_path.display());
                return Err(AppError::processing(
                    mark_recording_failed(&app, &state.db, recording, err_msg).await,
                ));
            }
        } else {
            let err_msg = format!("WAV file not found: {}", recording.audio_path.display());
            return Err(AppError::processing(
                mark_recording_failed(&app, &state.db, recording, err_msg).await,
            ));
        }
    };

    // Load and decode the WAV file on a blocking thread (CPU-intensive for large files).
    let wav_path_clone = wav_path.clone();
    let audio =
        match tokio::task::spawn_blocking(move || load_wav_to_audio_data(&wav_path_clone)).await {
            Ok(Ok(audio)) => audio,
            Ok(Err(e)) => {
                let err_msg = unwrap_app_error_message(e);
                return Err(AppError::processing(
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
        let sum_sq: f64 = audio
            .samples
            .iter()
            .map(|s| (*s as f64) * (*s as f64))
            .sum();
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
        return Err(AppError::processing(
            mark_recording_failed(&app, &state.db, recording.clone(), err_msg).await,
        ));
    }

    // Build STT config from caller parameters.
    // Diarize: an explicit per-call override (the pipeline stage forwards
    // `None`; a direct transcribe command may forward `Some(_)`) falls back
    // to the persisted AppConfig `diarize` flag (Settings → Audio / STT),
    // which defaults to OFF.
    // Default language to the app's configured language (e.g. "en-US") when
    // the caller doesn't specify one — whisper.cpp's auto-detect is unreliable
    // on short clips and frequently misdetects as Chinese.
    let effective_language = language
        .or_else(|| Some(app_config.language.clone()))
        .filter(|l| !l.is_empty());
    let config = SttConfig {
        language: effective_language,
        diarize: diarize.unwrap_or(app_config.diarize),
        num_speakers: app_config.max_speakers,
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
                return Err(AppError::stt_provider(
                    mark_recording_failed(&app, &state.db, recording, err_msg).await,
                ));
            }
        }
    };
    let token = cancel.clone().unwrap_or_default();
    // Did the USER (per-call override or persisted config) ask for speaker
    // labels? Distinct from `config.diarize` being effective: the provider
    // may still skip the run when models are missing — that gap is what the
    // outcome distinction below reports on.
    let diarize_requested = config.diarize;
    let transcript = match stt.transcribe(audio, config, token).await {
        Ok(t) => t,
        Err(e) => {
            // Preserve EndpointOffline as-is so the frontend dialog can fire.
            // All other STT errors go through mark_recording_failed. The DB
            // row still flips to Failed (db-only, no "failed" progress event
            // — the offline dialog owns the UX) so it isn't stuck spinning
            // "Processing" until the next boot sweep.
            if matches!(e, AppError::EndpointOffline { .. }) {
                mark_recording_failed_db_only(
                    &state.db,
                    recording,
                    "Transcription failed: AI/STT endpoint offline".to_string(),
                )
                .await;
                return Err(e);
            }
            let err_msg = format!("Transcription failed: {e}");
            tracing::error!(error = %e, "STT transcription failed");
            return Err(AppError::processing(
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

    // Diarization outcome reporting (see `diarization_outcome` below for the
    // state definitions). A user who explicitly turned the setting on must
    // not silently get unlabeled text when something went wrong.
    match diarization_outcome(&transcript, diarize_requested) {
        DiarizationOutcome::NotRequested | DiarizationOutcome::Succeeded => {}
        DiarizationOutcome::Skipped => {
            tracing::warn!(
                recording_id = %recording_id,
                "Diarization requested but skipped (models missing) — emitting diarization-warning"
            );
            let _ = app.emit("diarization-warning", &recording_id);
        }
        DiarizationOutcome::Failed(reason) => {
            tracing::error!(
                recording_id = %recording_id,
                error = %reason,
                "Diarization was requested and attempted but FAILED — emitting diarization-failed"
            );
            let _ = app.emit("diarization-failed", &recording_id);
        }
        DiarizationOutcome::SucceededNoSpeakers => {
            // Attempted, did not fail, and every segment is unlabeled: the
            // turns list came back empty. Log at info — this is the expected
            // single-speaker collapse, not a failure.
            tracing::info!(
                recording_id = %recording_id,
                "Diarization ran but produced no speaker turns (single-speaker collapse) — no labels"
            );
        }
    }

    // Collapse whisper.cpp repetition loops ("okay okay okay all right all
    // right all right") before formatting. This is a decoding pathology, not
    // real speech; the filter conservatively only collapses when a short
    // token pattern repeats 3+ times and dominates the segment.
    let mut transcript = transcript;
    filter_segment_repetitions(&mut transcript);
    // Drop runs of 3+ consecutive identical short segments — whisper.cpp
    // trailing-silence hallucination (e.g., 6× separate "so" segments after
    // the conversation ends). Rebuilds transcript.text from survivors.
    filter_cross_segment_repetitions(&mut transcript);

    // Build speaker-attributed text when diarization segments are available.
    let display_text = format_transcript_with_speakers(&transcript);

    // Guard: silent source + repeated-phrase output is a Whisper hallucination.
    // Rejecting here stops us from generating a bogus SOAP from nonsense like
    // "Thank you. Thank you. Thank you. ..." that Whisper emits on silence.
    if rms < 0.001 && is_repeated_phrase_hallucination(&transcript.text) {
        let err_msg = format!(
            "Transcription rejected: the audio was effectively silent (rms={rms:.6}) and the model returned a repeated-phrase hallucination. Check your microphone or audio routing."
        );
        // PHI guardrail: log structural metadata only — never the transcript text.
        tracing::warn!(
            provider = %transcript.provider,
            rms = %format!("{:.6}", rms),
            text_len = transcript.text.chars().count(),
            segments = transcript.segments.len(),
            "Rejecting likely Whisper hallucination from silent source"
        );
        return Err(AppError::processing(
            mark_recording_failed(&app, &state.db, recording, err_msg).await,
        ));
    }

    // Guard: if transcription produced no text, mark as Failed rather than
    // silently storing an empty transcript as "Completed".
    if display_text.trim().is_empty() {
        let err_msg = "Transcription produced no text — the recording may be silent or too short."
            .to_string();
        tracing::warn!(
            provider = %transcript.provider,
            segments = transcript.segments.len(),
            "{err_msg}"
        );
        return Err(AppError::processing(
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
    // clients. On failure (server unreachable, transient HTTP error), fall
    // back to the LOCAL rules (warned in the log) rather than aborting the
    // whole transcription or shipping an uncorrected transcript —
    // corrections are best-effort polish on top of the already-successful
    // STT output.
    let vocab_enabled = {
        let db_settings = Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || -> bool {
            let conn = match db_settings.conn() {
                Ok(c) => c,
                Err(_) => return true,
            };
            SettingsRepo::load_config(&conn)
                .ok()
                .map(|mut c| {
                    c.migrate();
                    c.vocabulary_enabled
                })
                .unwrap_or(true)
        })
        .await
        .unwrap_or(true)
    };

    let remote_entries: Option<Vec<medical_core::types::vocabulary::VocabularyEntry>> =
        if vocab_enabled {
            if let Some(conn) = crate::state::load_paired_connection_offload().await {
                if conn.ports.vocab.is_some() {
                    let bearer = crate::state::load_sharing_bearer_offload().await;
                    if let Some(remote) = crate::vocab_remote::VocabRemote::from(
                        &conn,
                        bearer,
                        state.http_client.clone(),
                    ) {
                        match remote.list(None).await {
                            Ok(list) => Some(list.into_iter().filter(|e| e.enabled).collect()),
                            Err(e) => {
                                tracing::warn!(
                                    "remote vocab fetch failed; skipping corrections: {e}"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

    let db_vocab = Arc::clone(&state.db);
    let (display_text, transcript) = match tokio::task::spawn_blocking(move || {
        if !vocab_enabled {
            return Ok::<_, AppError>((display_text, transcript));
        }
        let entries = match remote_entries {
            Some(remote) => remote,
            None => {
                // Paired but the server rules are unavailable (fetch failed,
                // or the server predates the vocab API). Fall back to the
                // local rules rather than skipping — some corrections beat
                // none — and log loudly so the staleness is visible.
                if crate::state::load_paired_connection().is_some() {
                    tracing::warn!(
                        "vocabulary: server rules unavailable (fetch failed or server predates the vocab API) — falling back to local rules, which may be stale"
                    );
                }
                let conn = db_vocab.conn()?;
                VocabularyRepo::list_enabled(&conn)?
            }
        };
        if entries.is_empty() {
            return Ok((display_text, transcript));
        }
        // Correct BOTH representations: each segment's text (the
        // speaker-labeled editor view reads transcript_segments from
        // metadata) AND the flat text — format_transcript_with_speakers
        // falls back to the FLAT text whenever no segment carries a
        // speaker label (diarization off / no speakers detected), and
        // corrections applied to segments alone were silently discarded
        // on that path.
        let total_replacements = vocabulary_corrector::apply_to_transcript(&mut transcript, &entries);
        if total_replacements > 0 {
            tracing::info!(
                replacements = total_replacements,
                "Applied vocabulary corrections to transcript"
            );
        }
        let display_text = format_transcript_with_speakers(&transcript);
        Ok((display_text, transcript))
    })
    .await
    {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            let err_msg = unwrap_app_error_message(e);
            return Err(AppError::processing(
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
    // `Processing` state. The transcript text is written to an orphaned-
    // transcript recovery file (not the log) so operators can recover it
    // manually; only the path is logged.
    recording.transcript = Some(display_text.clone());
    recording.stt_provider = Some(transcript.provider.clone());
    recording.status = ProcessingStatus::Completed {
        completed_at: Utc::now(),
    };

    // Store structured segment data (with speaker labels and timestamps) in
    // the recording's metadata JSON so the frontend can render a rich speaker
    // display without re-parsing the formatted text. Applied as a persist-time
    // PATCH (merged into the row's CURRENT metadata) — see the producer
    // persist below.
    let segments_json: serde_json::Value = serde_json::Value::Array(
        transcript
            .segments
            .iter()
            .map(|s| {
                serde_json::json!({
                    "speaker": s.speaker,
                    "text": s.text,
                    "start": s.start,
                    "end": s.end,
                })
            })
            .collect(),
    );

    let recording_for_failure = recording.clone();
    let persist_id = recording.id;
    let persist_transcript = display_text.clone();
    let persist_stt_provider = transcript.provider.clone();
    let persist_status_json = serde_json::to_string(&ProcessingStatus::Completed {
        completed_at: Utc::now(),
    })
    .unwrap_or_else(|_| "\"pending\"".to_string());
    let join_result = tokio::task::spawn_blocking({
        let db = Arc::clone(&state.db);
        move || {
            let conn = db.conn()?;
            // Targeted producer persist: the recording snapshot above is
            // minutes stale by now — a whole-row update would revert any
            // column the editor (or a concurrent generator) saved while
            // transcription ran. Only the transcription-owned columns are
            // written; the segments patch merges into current metadata.
            RecordingsRepo::persist_producer_update(
                &conn,
                &persist_id,
                &medical_db::recordings::ProducerPersist {
                    transcript: Some(persist_transcript),
                    stt_provider: Some(persist_stt_provider),
                    processing_status: Some(persist_status_json),
                    metadata_patch: vec![("transcript_segments".into(), segments_json)],
                    ..Default::default()
                },
            )?;
            Ok::<(), AppError>(())
        }
    })
    .await;

    match join_result {
        Ok(Ok(())) => { /* success — fall through to emit */ }
        Ok(Err(db_err)) => {
            let inner = unwrap_app_error_message(db_err);
            let err_msg =
                format!("Transcription succeeded but failed to persist final status: {inner}");
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
            return Err(AppError::database(
                mark_recording_failed(&app, &state.db, recording_for_failure, err_msg).await,
            ));
        }
        Err(join_err) => {
            let err_msg =
                format!("Transcription succeeded but DB persist task panicked: {join_err}");
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
/// Format a transcript with speaker labels and per-segment timestamps.
///
/// Each whisper segment is emitted as its own block, with an SRT-style
/// timestamp and a bracketed speaker label:
///
/// ```text
/// 00:00:01,340 --> 00:00:03,750 [Speaker 1]
/// Good, good. You need some refills today.
/// ```
///
/// This preserves the segment granularity that whisper.cpp produces (one
/// segment per clause/pause), unlike the previous "one paragraph per
/// speaker" format that collapsed all of a speaker's consecutive text into
/// a single run-on blob.
///
/// Falls back to the raw text when no speaker segments are present at all.
fn format_transcript_with_speakers(transcript: &medical_core::types::stt::Transcript) -> String {
    let any_speakers = transcript.segments.iter().any(|s| s.speaker.is_some());
    if !any_speakers {
        return transcript.text.clone();
    }

    let mut result = String::new();
    let mut last_speaker: Option<&str> = None;

    for seg in &transcript.segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        // Fold unlabeled segments into the last known speaker so no text
        // is dropped (diarization may not cover every whisper segment).
        let speaker = seg.speaker.as_deref().or(last_speaker);
        if speaker.is_some() {
            last_speaker = speaker;
        }

        if !result.is_empty() {
            result.push_str("\n\n");
        }

        // SRT-style timestamp: HH:MM:SS,mmm --> HH:MM:SS,mmm
        result.push_str(&format_srt_timestamp(seg.start, seg.end));
        result.push(' ');

        // Bracketed speaker label: [Speaker 1]
        //
        // NUMBERING CONVENTION (B3): "Speaker N" is 1-based everywhere —
        // merge.rs already emits `format!("Speaker {}", id + 1)`, the rich
        // view badges render the metadata label verbatim, and this stored /
        // copied text must match. The old `n - 1` here produced a 0-based
        // off-by-one against every other surface.
        // Labels are neutral speaker ordinals with NO clinical role
        // inference — the diarizer cannot know who is the doctor and who
        // is the patient, and must never label as such (see
        // docs/design/speaker-labelling.md).
        if let Some(label) = speaker {
            result.push_str(&format!("[{label}] "));
        }

        result.push('\n');
        result.push_str(text);
    }

    result
}

/// Format a start/end timestamp pair (in seconds) as an SRT-style range:
/// `00:00:01,340 --> 00:00:03,750`
fn format_srt_timestamp(start: f64, end: f64) -> String {
    format!("{} --> {}", format_srt_time(start), format_srt_time(end))
}

/// Format a single timestamp (in seconds) as `HH:MM:SS,mmm`.
fn format_srt_time(t: f64) -> String {
    let total_ms = (t * 1000.0).round() as u64;
    let hours = total_ms / 3_600_000;
    let minutes = (total_ms % 3_600_000) / 60_000;
    let seconds = (total_ms % 60_000) / 1_000;
    let millis = total_ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// Outcome of the (optional) speaker-diarization stage, reconstructed from
/// the provider's transcript metadata. Distinguishes the states a user who
/// explicitly enabled diarization must not have silently conflated:
///
/// - `NotRequested` — diarize is off (default); nothing to report.
/// - `Skipped` — requested, but the provider never attempted a run because
///   the pyannote models are absent (`diarization_attempted == false`).
///   Degrades to unlabeled text; surfaced as `diarization-warning`.
/// - `Failed` — requested and attempted, but the run errored or panicked.
///   The providers deliberately swallow the error so the transcription
///   still completes, but report it via metadata `diarization_failed`
///   (added in this change; previously it was only a provider-side `warn!`
///   log invisible to this call-site). Surfaced as `diarization-failed`.
/// - `SucceededNoSpeakers` — attempted, did NOT fail, and no segment got a
///   speaker label: the diarizer found zero speaker turns, i.e. a
///   single-speaker recording collapsed to no labels. NOT a failure.
/// - `Succeeded` — at least one segment carries a speaker label.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DiarizationOutcome {
    NotRequested,
    Skipped,
    Failed(String),
    SucceededNoSpeakers,
    Succeeded,
}

/// Classify the diarization outcome from the transcript the provider
/// returned. Pure function over (metadata, segments) so it is directly
/// unit-testable without a Tauri runtime.
pub(crate) fn diarization_outcome(
    transcript: &medical_core::types::stt::Transcript,
    diarize_requested: bool,
) -> DiarizationOutcome {
    if !diarize_requested {
        return DiarizationOutcome::NotRequested;
    }
    // Attempted: models were present and the provider invoked the diarizer.
    let attempted = transcript
        .metadata
        .get("diarization_attempted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !attempted {
        return DiarizationOutcome::Skipped;
    }
    // Failed: the run was attempted and errored/panicked; the provider
    // swallowed the error to keep the transcription alive but reported it.
    if let Some(reason) = transcript
        .metadata
        .get("diarization_failed")
        .filter(|v| !v.is_null())
    {
        let reason = match reason.as_str() {
            Some(s) => s.to_string(),
            None => reason.to_string(),
        };
        return DiarizationOutcome::Failed(reason);
    }
    // Attempted and not failed — did any segment get a label?
    if transcript.segments.iter().any(|s| s.speaker.is_some()) {
        DiarizationOutcome::Succeeded
    } else {
        DiarizationOutcome::SucceededNoSpeakers
    }
}


#[cfg(test)]
mod format_tests {
    use medical_core::types::stt::{Transcript, TranscriptSegment};

    fn make_transcript(segments: Vec<TranscriptSegment>) -> Transcript {
        let text = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Transcript {
            text,
            segments,
            language: None,
            duration_seconds: None,
            provider: "test".into(),
            metadata: serde_json::json!({}),
        }
    }

    fn seg(text: &str, speaker: Option<&str>) -> TranscriptSegment {
        TranscriptSegment {
            text: text.into(),
            start: 0.0,
            end: 1.0,
            speaker: speaker.map(Into::into),
            confidence: None,
        }
    }

    #[test]
    fn no_speakers_returns_raw_text() {
        let t = make_transcript(vec![seg("Hello", None), seg("World", None)]);
        assert_eq!(super::format_transcript_with_speakers(&t), "Hello World");
    }

    #[test]
    fn all_labeled_groups_by_speaker() {
        let t = make_transcript(vec![
            seg("Hi", Some("Speaker 1")),
            seg("there", Some("Speaker 1")),
            seg("Hello", Some("Speaker 2")),
        ]);
        let result = super::format_transcript_with_speakers(&t);
        // Each segment gets its own SRT-style block with timestamp + speaker.
        // B3: labels are 1-based and pass through UNCHANGED from merge.rs
        // ("Speaker 1"/"Speaker 2") — stored text must match the rich view.
        assert!(
            result.contains("[Speaker 1]"),
            "first speaker; got: {result}"
        );
        assert!(
            result.contains("[Speaker 2]"),
            "second speaker; got: {result}"
        );
        assert!(!result.contains("[Speaker 0]"), "no 0-based labels; got: {result}");
        assert!(result.contains("Hi"), "first text; got: {result}");
        assert!(result.contains("Hello"), "third text; got: {result}");
        assert!(result.contains("-->"), "timestamps present; got: {result}");
    }

    #[test]
    fn unlabeled_segments_included_not_dropped() {
        let t = make_transcript(vec![
            seg("Doctor speaking", Some("Speaker 1")),
            seg("mm-hmm", None), // unlabeled — must NOT be dropped
            seg("I see", Some("Speaker 2")),
        ]);
        let result = super::format_transcript_with_speakers(&t);
        assert!(
            result.contains("mm-hmm"),
            "unlabeled segment must appear in output; got: {result}"
        );
        assert!(result.contains("[Speaker 1]"));
        assert!(result.contains("[Speaker 2]"));
    }

    #[test]
    fn unlabeled_before_first_speaker_emitted_without_label() {
        let t = make_transcript(vec![
            seg("Background noise", None),
            seg("Hello", Some("Speaker 1")),
        ]);
        let result = super::format_transcript_with_speakers(&t);
        assert!(
            result.contains("Background noise"),
            "unlabeled prefix must appear; got: {result}"
        );
        assert!(
            result.contains("[Speaker 1]"),
            "labeled segment follows; got: {result}"
        );
    }

    #[test]
    fn unlabeled_after_speaker_folded_into_that_speaker() {
        let t = make_transcript(vec![
            seg("Take this", Some("Speaker 1")),
            seg("okay", None), // should fold into Speaker 1
            seg("Thanks", Some("Speaker 2")),
        ]);
        let result = super::format_transcript_with_speakers(&t);
        // "okay" appears and is attributed to [Speaker 1] (the last known).
        assert!(
            result.contains("okay"),
            "folded text present; got: {result}"
        );
        assert!(
            result.contains("[Speaker 1]"),
            "folded into Speaker 1; got: {result}"
        );
        assert!(
            result.contains("[Speaker 2]"),
            "speaker change after fold; got: {result}"
        );
    }

    #[test]
    fn empty_segments_handled() {
        let t = make_transcript(vec![]);
        assert_eq!(super::format_transcript_with_speakers(&t), "");
    }

    /// B8 regression: a single-speaker collapse — diarization attempted,
    /// succeeded, but produced ZERO speaker turns (empty turns merge into
    /// unlabeled segments) — must produce NO speaker labels and NO brackets.
    #[test]
    fn single_speaker_collapse_yields_no_labels() {
        let t = make_transcript(vec![seg("Solo monologue", None), seg("part two", None)]);
        let result = super::format_transcript_with_speakers(&t);
        assert_eq!(result, "Solo monologue part two");
        assert!(!result.contains('['), "no labels expected; got: {result}");
    }

    /// B8 regression: an explicitly-requested diarization run that the
    /// provider skipped (models missing) classifies as Skipped.
    #[test]
    fn diarization_outcome_skipped_when_models_missing() {
        let t = make_transcript(vec![seg("Hi", None)]);
        assert_eq!(
            super::diarization_outcome(&t, true),
            super::DiarizationOutcome::Skipped
        );
    }

    /// B8 regression: an explicitly-requested run that errored mid-flight
    /// (provider swallowed the error, reported it via metadata) classifies
    /// as Failed and carries the reason.
    #[test]
    fn diarization_outcome_failed_when_provider_reports_failure() {
        let mut t = make_transcript(vec![seg("Hi", None)]);
        t.metadata = serde_json::json!({
            "diarization_attempted": true,
            "diarization_failed": "diarization failed: model corrupted",
        });
        assert_eq!(
            super::diarization_outcome(&t, true),
            super::DiarizationOutcome::Failed(
                "diarization failed: model corrupted".to_string()
            )
        );
    }

    /// B8 regression: an explicitly-requested run that attempted, did not
    /// fail, but found no speakers (single-speaker collapse) classifies as
    /// SucceededNoSpeakers — NOT Failed, NOT Skipped.
    #[test]
    fn diarization_outcome_single_speaker_collapse_is_not_a_failure() {
        let mut t = make_transcript(vec![seg("Solo", None)]);
        t.metadata = serde_json::json!({
            "diarization_attempted": true,
            "diarization_failed": null,
        });
        assert_eq!(
            super::diarization_outcome(&t, true),
            super::DiarizationOutcome::SucceededNoSpeakers
        );
    }

    /// B8 regression: diarize off (the default) never reports anything.
    #[test]
    fn diarization_outcome_not_requested_when_off() {
        let mut t = make_transcript(vec![seg("Hi", Some("Speaker 1"))]);
        t.metadata = serde_json::json!({
            "diarization_attempted": false,
            "diarization_failed": null,
        });
        assert_eq!(
            super::diarization_outcome(&t, false),
            super::DiarizationOutcome::NotRequested
        );
    }

    /// Sanity: attempted + labeled segments = Succeeded.
    #[test]
    fn diarization_outcome_succeeded_with_labels() {
        let mut t = make_transcript(vec![seg("Hi", Some("Speaker 1"))]);
        t.metadata = serde_json::json!({
            "diarization_attempted": true,
            "diarization_failed": null,
        });
        assert_eq!(
            super::diarization_outcome(&t, true),
            super::DiarizationOutcome::Succeeded
        );
    }
}

/// Integration tests for the pre-flight gate added by Task 8.
///
/// `transcribe_recording_inner` takes a `tauri::AppHandle` which cannot be
/// easily constructed outside the Tauri runtime.  The tests below validate the
/// pre-flight path in isolation: they call `preflight_for_command` directly
/// with an `AppConfig` that matches what the inner function loads from the DB,
/// confirming that the gate fires correctly for the Transcribe kind.
#[cfg(test)]
mod preflight_tests {
    use medical_core::error::{AppError, OfflineReason, ServiceKind};
    use medical_core::preflight::{CommandKind, preflight_for_command};
    use medical_core::types::settings::AppConfig;

    /// Verify that a non-empty, non-loopback `stt_remote_host` triggers a probe
    /// and produces `EndpointOffline` when the host is unreachable.
    #[tokio::test]
    async fn transcribe_preflight_returns_endpoint_offline_when_stt_unreachable() {
        // 192.0.2.1 is RFC 5737 TEST-NET-1 — guaranteed unrouteable.
        let mut config = AppConfig::default();
        config.stt_remote_host = "192.0.2.1".to_string();
        config.stt_remote_port = 8080;

        let start = std::time::Instant::now();
        let result = preflight_for_command(CommandKind::Transcribe, &config).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("unrouteable STT host must fail preflight");
        match err {
            AppError::EndpointOffline {
                service, reason, ..
            } => {
                assert_eq!(service, ServiceKind::RemoteStt);
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

    /// Verify that an empty `stt_remote_host` (local whisper) skips the probe.
    #[tokio::test]
    async fn transcribe_preflight_skips_when_stt_remote_host_is_empty() {
        let mut config = AppConfig::default();
        config.stt_remote_host = String::new(); // local whisper — no probe needed
        config.stt_remote_port = 8080;

        let result = preflight_for_command(CommandKind::Transcribe, &config).await;
        assert!(
            result.is_ok(),
            "empty stt_remote_host must skip preflight; got {result:?}"
        );
    }

    /// Regression test: a pre-flight failure must NOT leave the recording stuck
    /// in `Processing` status.
    ///
    /// `transcribe_recording_inner` requires a `tauri::AppHandle` which can't be
    /// constructed in unit tests. This test validates the invariant directly: it
    /// simulates the pre-flight-before-status-write ordering by running the
    /// pre-flight against an unrouteable host and confirming the error fires
    /// BEFORE any DB mutation would occur. A recording inserted as `Pending`
    /// must still be `Pending` after the pre-flight returns `EndpointOffline`.
    #[tokio::test]
    async fn transcribe_preflight_failure_does_not_leave_recording_processing() {
        use medical_core::types::recording::{ProcessingStatus, Recording};
        use medical_db::Database;
        use medical_db::recordings::RecordingsRepo;
        use std::path::PathBuf;

        // Build an in-memory DB and insert a Pending recording.
        let db = std::sync::Arc::new(Database::open_in_memory().expect("open in-memory db"));
        let recording = {
            let rec = Recording::new("test.wav", PathBuf::from("/tmp/test.wav"));
            // Status is Pending by default (Recording::new).
            let conn = db.conn().expect("conn");
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            rec
        };
        let recording_id = recording.id;

        // Configure an unrouteable STT host — pre-flight will return EndpointOffline.
        let mut config = AppConfig::default();
        config.stt_remote_host = "192.0.2.1".to_string();
        config.stt_remote_port = 8080;

        // Run pre-flight (mirrors what transcribe_recording_inner does BEFORE
        // the Processing status write after the reorder fix).
        let preflight_result = preflight_for_command(CommandKind::Transcribe, &config).await;

        // Pre-flight must have failed.
        let err = preflight_result.expect_err("unrouteable host must fail preflight");
        assert!(
            matches!(err, AppError::EndpointOffline { .. }),
            "expected EndpointOffline, got {err:?}"
        );

        // The recording must NOT be Processing — it was never mutated.
        let conn = db.conn().expect("conn");
        let loaded = RecordingsRepo::get_by_id(&conn, &recording_id).expect("get");
        assert!(
            !matches!(loaded.status, ProcessingStatus::Processing { .. }),
            "recording must not be Processing after pre-flight failure; status = {:?}",
            loaded.status
        );
    }
}
