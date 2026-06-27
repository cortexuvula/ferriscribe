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
            "soap" => {
                PdfExporter::export_soap(&recording).map_err(|e| AppError::Export(e.to_string()))
            }
            "referral" => PdfExporter::export_referral(&recording)
                .map_err(|e| AppError::Export(e.to_string())),
            "letter" => {
                PdfExporter::export_letter(&recording).map_err(|e| AppError::Export(e.to_string()))
            }
            other => Err(AppError::Export(format!("Unknown export type: {other}"))),
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
            "soap" => {
                DocxExporter::export_soap(&recording).map_err(|e| AppError::Export(e.to_string()))
            }
            "referral" => DocxExporter::export_referral(&recording)
                .map_err(|e| AppError::Export(e.to_string())),
            "letter" => {
                DocxExporter::export_letter(&recording).map_err(|e| AppError::Export(e.to_string()))
            }
            other => Err(AppError::Export(format!("Unknown export type: {other}"))),
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
        .map_err(|e| AppError::Export(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("export task failed: {e}")))?
}
