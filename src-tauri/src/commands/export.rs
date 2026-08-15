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
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let recording = load_recording_blocking(&db, &recording_id)?;

        // Decrypt the recording (handles both encrypted FE1 and legacy plaintext).
        let wav_bytes =
            crate::commands::transcription::helpers::open_recording_wav_raw(&recording.audio_path)?;

        // Parse the decrypted WAV to get sample format + data.
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav_bytes))
            .map_err(|e| AppError::audio(format!("Failed to parse WAV: {e}")))?;
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
        let mut writer = hound::WavWriter::create(&file_path, out_spec)
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
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}
