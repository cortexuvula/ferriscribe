use std::path::PathBuf;

use medical_core::error::{AppError, AppResult};
use medical_core::types::recording::{ProcessingStatus, Recording, RecordingSummary};
use medical_db::recordings::RecordingsRepo;
use medical_db::search::SearchRepo;
use medical_db::vectors::VectorsRepo;
use uuid::Uuid;

use super::resolve_recordings_dir;
use crate::state::AppState;

/// List recordings with optional pagination.
///
/// Returns up to `limit` (default 50) recordings starting at `offset` (default 0),
/// ordered by creation date descending.
#[tauri::command]
pub fn list_recordings(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<RecordingSummary>> {
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    RecordingsRepo::list_all(&conn, limit.unwrap_or(50), offset.unwrap_or(0))
        .map_err(|e| AppError::Database(e.to_string()))
}

/// Get a single recording by its UUID.
///
/// Returns the full `Recording` including transcript, SOAP note, and metadata.
#[tauri::command]
pub fn get_recording(state: tauri::State<'_, AppState>, id: String) -> AppResult<Recording> {
    let uuid =
        Uuid::parse_str(&id).map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))
}

/// Full-text search across recording transcripts and SOAP notes.
///
/// Returns up to `limit` (default 20) matching recordings.
#[tauri::command]
pub fn search_recordings(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> AppResult<Vec<Recording>> {
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    SearchRepo::search_recordings(&conn, &query, limit.unwrap_or(20))
        .map_err(|e| AppError::Database(e.to_string()))
}

/// Delete a recording by UUID.
///
/// Removes the DB row, associated RAG vectors, and the WAV file from disk.
/// The DB delete and vector cleanup are atomic (same transaction); the WAV
/// file is removed only after the transaction commits.
#[tauri::command]
pub fn delete_recording(state: tauri::State<'_, AppState>, id: String) -> AppResult<()> {
    let uuid =
        Uuid::parse_str(&id).map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let mut conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;

    // Get the recording first so we can clean up the WAV file. Distinguish a
    // genuine DB error (propagate) from "not found" (proceed — the caller's
    // intent is to delete, so an already-absent row is a no-op success). The
    // old `.ok()` swallowed DB errors as None, silently orphaning the WAV.
    let recording = match RecordingsRepo::get_by_id(&conn, &uuid) {
        Ok(r) => Some(r),
        Err(medical_db::DbError::NotFound(_)) => None,
        Err(e) => return Err(AppError::Database(e.to_string())),
    };

    // Delete the vectors and the recording row atomically: if either fails,
    // rolling back leaves the user in a consistent "still present" state they
    // can retry, rather than orphaned vectors pointing at a deleted recording.
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("begin tx: {e}")))?;
    delete_rag_vectors_best_effort(&tx, &id);
    RecordingsRepo::delete(&tx, &uuid).map_err(|e| AppError::Database(e.to_string()))?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("commit tx: {e}")))?;

    // Delete the WAV file from disk only after the DB commit succeeds —
    // removing the file first and failing the DB delete would leave a row
    // pointing at nothing.
    if let Some(rec) = recording
        && rec.audio_path.exists()
        && let Err(e) = std::fs::remove_file(&rec.audio_path)
    {
        tracing::warn!(path = %rec.audio_path.display(), error = %e, "WAV delete failed");
    }

    Ok(())
}

/// Delete RAG vectors for a recording, logging failures rather than aborting
/// the recording deletion. Intentional: users expect recording delete to
/// succeed even if the vector index is temporarily unreachable or the chunks
/// were never persisted in the first place. Orphaned vectors are a known
/// tradeoff — a follow-up background task should eventually sweep them.
///
/// Uses `tracing::error!` (not `warn!`) so orphans are visible in operations
/// dashboards even when warn is filtered out.
fn delete_rag_vectors_best_effort(conn: &medical_db::Connection, recording_id: &str) {
    if let Err(e) = VectorsRepo::delete_by_document(conn, recording_id) {
        tracing::error!(
            recording_id = %recording_id,
            error = %e,
            "Failed to delete RAG vectors during recording delete; vectors may be orphaned until a future cleanup pass"
        );
    }
}

/// Delete all recordings and their associated data.
///
/// Removes all recording rows, RAG vectors, and WAV files from disk.
/// Returns the number of recordings deleted.
#[tauri::command]
pub fn delete_all_recordings(state: tauri::State<'_, AppState>) -> AppResult<u32> {
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;

    // Delete all RAG vectors
    if let Err(e) = conn.execute("DELETE FROM vectors", []) {
        tracing::error!(error = %e, "RAG vector cleanup failed during delete_all_recordings; orphan vectors may remain");
    }

    // Delete all recordings and get audio paths for file cleanup
    let paths = RecordingsRepo::delete_all(&conn).map_err(|e| AppError::Database(e.to_string()))?;
    let count = paths.len() as u32;

    // Remove audio files from disk
    for path in &paths {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    Ok(count)
}

/// Import an audio file from the filesystem into the recordings library.
///
/// Non-WAV files (MP3, FLAC, OGG, M4A, AAC) are automatically converted to
/// WAV so the transcription pipeline can process them.  Creates a Recording
/// entry in the database and returns the new recording ID.
#[tauri::command]
pub fn import_audio_file(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<String> {
    let source = PathBuf::from(&file_path);
    if !source.exists() {
        return Err(AppError::Other(format!("File not found: {file_path}")));
    }

    // Resolve recordings directory from settings (custom path or default).
    let recordings_dir = resolve_recordings_dir(&state.db, &state.data_dir)?;

    let original_name = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".to_string());

    let recording_id = Uuid::new_v4();
    let short_id = &recording_id.to_string()[..8];

    // Determine if we need to convert to WAV.
    let dest_path = if medical_audio::convert::is_wav_file(&source) {
        // Already WAV — just copy.
        let dest_filename = format!("{original_name}_{short_id}.wav");
        let dest = recordings_dir.join(&dest_filename);
        std::fs::copy(&source, &dest)
            .map_err(|e| AppError::Audio(format!("Failed to copy file: {e}")))?;
        dest
    } else {
        // Non-WAV — convert to WAV.
        let dest_filename = format!("{original_name}_{short_id}.wav");
        let dest = recordings_dir.join(&dest_filename);
        medical_audio::convert::convert_to_wav(&source, &dest)
            .map_err(|e| AppError::Audio(format!("Failed to convert audio: {e}")))?;
        dest
    };

    // Read duration and file size from the resulting WAV.
    let file_size = std::fs::metadata(&dest_path).map(|m| m.len()).ok();
    let duration = hound::WavReader::open(&dest_path).ok().map(|reader| {
        let spec = reader.spec();
        let total_samples = reader.len() as f64;
        if spec.sample_rate > 0 && spec.channels > 0 {
            total_samples / (spec.sample_rate as f64 * spec.channels as f64)
        } else {
            0.0
        }
    });

    let dest_filename = dest_path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{original_name}_{short_id}.wav"));

    // Create the Recording entry.
    let mut recording = Recording::new(dest_filename, dest_path);
    recording.id = recording_id;
    recording.duration_seconds = duration;
    recording.file_size_bytes = file_size;
    recording.status = ProcessingStatus::Pending;

    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;
    RecordingsRepo::insert(&conn, &recording).map_err(|e| AppError::Database(e.to_string()))?;

    Ok(recording_id.to_string())
}
