use medical_core::error::{AppError, AppResult};
use medical_db::recordings::RecordingsRepo;
use medical_export::docx::DocxExporter;
use medical_export::fhir::{FhirExporter, PatientInfo, PractitionerInfo};
use medical_export::pdf::PdfExporter;
use uuid::Uuid;

use crate::state::AppState;

#[tauri::command]
pub fn export_pdf(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    export_type: String,
) -> AppResult<Vec<u8>> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    let recording =
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))?;

    match export_type.as_str() {
        "soap" => PdfExporter::export_soap(&recording).map_err(|e| AppError::Export(e.to_string())),
        "referral" => {
            PdfExporter::export_referral(&recording).map_err(|e| AppError::Export(e.to_string()))
        }
        "letter" => {
            PdfExporter::export_letter(&recording).map_err(|e| AppError::Export(e.to_string()))
        }
        other => Err(AppError::Export(format!("Unknown export type: {other}"))),
    }
}

#[tauri::command]
pub fn export_docx(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    export_type: String,
) -> AppResult<Vec<u8>> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    let recording =
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))?;

    match export_type.as_str() {
        "soap" => {
            DocxExporter::export_soap(&recording).map_err(|e| AppError::Export(e.to_string()))
        }
        "referral" => {
            DocxExporter::export_referral(&recording).map_err(|e| AppError::Export(e.to_string()))
        }
        "letter" => {
            DocxExporter::export_letter(&recording).map_err(|e| AppError::Export(e.to_string()))
        }
        other => Err(AppError::Export(format!("Unknown export type: {other}"))),
    }
}

#[tauri::command]
pub fn export_fhir(state: tauri::State<'_, AppState>, recording_id: String) -> AppResult<Vec<u8>> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    let recording =
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))?;

    FhirExporter::export_bundle(
        &recording,
        PatientInfo::default(),
        PractitionerInfo::default(),
    )
    .map_err(|e| AppError::Export(e.to_string()))
}
