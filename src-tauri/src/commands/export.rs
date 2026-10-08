use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_db::recordings::RecordingsRepo;
use medical_export::docx::DocxExporter;
use medical_export::fhir::{FhirExporter, PatientInfo, PractitionerInfo};
use medical_export::pdf::PdfExporter;
use uuid::Uuid;

use crate::state::AppState;

/// All three export commands do two slow things: a SQLite read and a CPU-heavy
/// render (PDF font layout / DOCX XML / FHIR serialization). Doing this on the
/// IPC thread stalls every other `invoke()` from the frontend. We offload both
/// to `spawn_blocking`, mirroring `commands/generation/helpers.rs:39`.
fn load_recording_blocking(
    db: &Arc<medical_db::Database>,
    recording_id: &str,
) -> AppResult<medical_core::types::recording::Recording> {
    let uuid = Uuid::parse_str(recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let conn = db.conn()?;
    RecordingsRepo::get_by_id(&conn, &uuid).map_err(AppError::from)
}

#[tauri::command]
pub async fn export_pdf(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    export_type: String,
) -> AppResult<Vec<u8>> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
        let recording = load_recording_blocking(&db, &recording_id)?;
        match export_type.as_str() {
            "soap" => PdfExporter::export_soap(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            "referral" => PdfExporter::export_referral(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            "letter" => PdfExporter::export_letter(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            other => Err(AppError::export(format!("Unknown export type: {other}"))),
        }
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}

#[tauri::command]
pub async fn export_docx(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    export_type: String,
) -> AppResult<Vec<u8>> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
        let recording = load_recording_blocking(&db, &recording_id)?;
        match export_type.as_str() {
            "soap" => DocxExporter::export_soap(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            "referral" => DocxExporter::export_referral(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            "letter" => DocxExporter::export_letter(&recording)
                .map_err(|e| AppError::export_with_source(e.to_string(), e)),
            other => Err(AppError::export(format!("Unknown export type: {other}"))),
        }
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}

#[tauri::command]
pub async fn export_fhir(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<Vec<u8>> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
        let recording = load_recording_blocking(&db, &recording_id)?;
        FhirExporter::export_bundle(
            &recording,
            PatientInfo::default(),
            PractitionerInfo::default(),
        )
        .map_err(|e| AppError::export_with_source(e.to_string(), e))
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}

/// Export the audio recording as a standard 16-bit PCM WAV file.
///
/// Decrypts the at-rest encrypted recording (FE1 format) and converts from
/// 32-bit float to 16-bit PCM — the universal WAV format readable by every
/// audio player, transcription tool, and medical software. The output is
/// ~4x smaller than the original float WAV.
#[tauri::command]
pub async fn export_audio(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    file_path: String,
) -> AppResult<()> {
    let file_path = crate::commands::validate_user_path(&file_path)?;
    // Mark the write window: this export decrypts PHI audio to a
    // PLAINTEXT WAV written incrementally to a user-chosen path — not
    // atomic, not cancellable — so the coordinated-quit path refuses to
    // exit while it runs (commands/quit.rs). RAII: decrements on every
    // exit path, including task panics.
    let _export = crate::commands::quit::track_file_export();
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let recording = load_recording_blocking(&db, &recording_id)?;
        write_pcm16_wav_export(&recording, std::path::Path::new(&file_path))
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}

/// Decode a recording's at-rest audio and re-encode it as a standard
/// 16-bit PCM WAV at `file_path`. The blocking half of `export_audio`,
/// extracted so the audio-decode path is unit-testable.
///
/// Opens via the SALVAGING `open_recording_wav` (not the raw-bytes
/// variant): a crash-recovered WAV whose data-chunk length was never
/// finalized is repaired on open, so the samples the transcription
/// pipeline sees are the samples exported — the raw reader saw a
/// 0-sample "valid" parse and exported a silent file with success
/// semantics.
pub(crate) fn write_pcm16_wav_export(
    recording: &medical_core::types::recording::Recording,
    file_path: &std::path::Path,
) -> AppResult<()> {
    // Decrypt the recording (handles both encrypted FE1 and legacy
    // plaintext) + salvage unfinalized/truncated headers.
    let reader =
        crate::commands::transcription::helpers::open_recording_wav(&recording.audio_path)?;
    let spec = reader.spec();

    // Convert samples to i16 regardless of source format.
    let samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| {
                s.map(|v| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>(),
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            if bits == 0 {
                return Err(AppError::audio("WAV has bits_per_sample=0".to_string()));
            }
            let scale = 1i64 << (bits - 1);
            reader
                .into_samples::<i32>()
                .map(|s| {
                    s.map(|v| (v as i64 * i16::MAX as i64 / scale) as i16)
                        .unwrap_or(0)
                })
                .collect::<Vec<_>>()
        }
    };

    // Write as standard 16-bit PCM WAV.
    let out_spec = hound::WavSpec {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(file_path, out_spec)
        .map_err(|e| AppError::audio(format!("Failed to create output WAV: {e}")))?;
    for &sample in &samples {
        writer
            .write_sample(sample)
            .map_err(|e| AppError::audio(format!("WAV write: {e}")))?;
    }
    writer
        .finalize()
        .map_err(|e| AppError::audio(format!("WAV finalize: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::Recording;
    use std::path::PathBuf;

    /// A small valid stereo 16-bit WAV built with hound — the same fixture
    /// shape the salvage tests in `transcription/helpers.rs` use.
    fn sample_wav_bytes() -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("writer");
            for i in 0..100 {
                writer.write_sample(i as i16).expect("l");
                writer.write_sample(i as i16).expect("r");
            }
            writer.finalize().expect("finalize");
        }
        cursor.into_inner()
    }

    /// Locate the start of the `data` chunk body in a RIFF blob (test aid).
    fn find_data_chunk_start(bytes: &[u8]) -> Option<usize> {
        let mut pos = 12;
        while pos + 8 <= bytes.len() {
            if &bytes[pos..pos + 4] == b"data" {
                return Some(pos + 8);
            }
            let len = u32::from_le_bytes([
                bytes[pos + 4],
                bytes[pos + 5],
                bytes[pos + 6],
                bytes[pos + 7],
            ]) as usize;
            pos += 8 + len + (len & 1);
        }
        None
    }

    /// Regression (2026-10-08 review): `export_audio` used the raw
    /// (non-salvaging) reader, so a crash-recovered WAV whose data-chunk
    /// length was never finalized — transcribable via the salvage —
    /// exported as a silent 0-sample WAV while reporting success.
    #[test]
    fn export_audio_salvages_unfinalized_zero_length_wav() {
        // Crash/force-quit before finalize(): the data-chunk length field
        // is still 0 but real audio follows (mirrors
        // salvage_recovers_zero_length_data_chunk's fixture).
        let mut wav = sample_wav_bytes();
        let data_start = find_data_chunk_start(&wav).expect("data chunk");
        wav[data_start - 4..data_start].copy_from_slice(&0u32.to_le_bytes());

        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("unfinalized.wav");
        std::fs::write(&src, &wav).expect("write plaintext legacy wav");

        let mut recording = Recording::new("visit.wav".to_string(), PathBuf::from("/tmp/x.wav"));
        recording.audio_path = src.clone();

        let out = tmp.path().join("exported.wav");
        write_pcm16_wav_export(&recording, &out).expect("export");

        let reader = hound::WavReader::open(&out).expect("exported wav parses");
        assert_eq!(
            reader.duration(),
            100,
            "all whole frames recovered — a 0-sample silent export means the salvage was skipped"
        );
    }

    /// Happy path: a well-formed WAV round-trips through the i16
    /// re-encoding with every frame intact.
    #[test]
    fn export_audio_converts_a_finalized_wav() {
        let wav = sample_wav_bytes();
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("finalized.wav");
        std::fs::write(&src, &wav).expect("write");

        let mut recording = Recording::new("visit.wav".to_string(), PathBuf::from("/tmp/x.wav"));
        recording.audio_path = src;

        let out = tmp.path().join("exported.wav");
        write_pcm16_wav_export(&recording, &out).expect("export");

        let reader = hound::WavReader::open(&out).expect("exported wav parses");
        assert_eq!(reader.duration(), 100);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
    }
}
