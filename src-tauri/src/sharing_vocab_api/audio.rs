//! Audio file GET/PUT handlers for the `/v1/content/audio/{recording_id}`
//! routes.
//!
//! These move encrypted audio bytes between paired clients and the office
//! server. GET decrypts (or reads legacy plaintext) on the way out; PUT
//! encrypts in memory and writes atomically on the way in. No PHI (audio
//! bytes) is ever logged — only ID lengths and byte counts.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use medical_db::recordings::RecordingsRepo;
use medical_security::file_crypto;
use tracing::{info, warn};
use uuid::Uuid;

// Re-export tauri::Emitter so the inline `state.app_handle.emit(...)` call
// below compiles without importing the trait into the module namespace
// (mirrors the pattern in `content_sync.rs`).
use tauri::Emitter as _;

use super::{ApiState, authorize};

/// GET /v1/content/audio/{recording_id} — download decrypted audio bytes.
///
/// Loads the recording row, resolves its `audio_path`, and decrypts the
/// file (falling back to plaintext read for legacy unencrypted files). The
/// raw bytes are returned as the response body. Only the ID length and byte
/// count are logged — never the content.
pub(super) async fn content_audio_get_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
) -> Result<Response, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let id_len = recording_id.len();
    let db = Arc::clone(&state.db);
    let data_dir = state.data_dir.clone();

    let bytes =
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, medical_core::error::AppError> {
            let conn = db.conn()?;
            let uuid = Uuid::parse_str(&recording_id)
                .map_err(|_| medical_core::error::AppError::Other("invalid recording id".into()))?;
            let rec = RecordingsRepo::get_by_id(&conn, &uuid)
                .map_err(medical_core::error::AppError::from)?;
            let path = &rec.audio_path;
            if path.as_os_str().is_empty() || !path.exists() {
                return Err(medical_core::error::AppError::Other(
                    "audio file not found".into(),
                ));
            }
            // Containment check: verify the audio path is within the
            // recordings directory. This prevents a malicious DB value
            // from causing the server to decrypt/read arbitrary files.
            let recordings_dir = crate::commands::resolve_recordings_dir(&db, &data_dir)?;
            let canonical_path = path.canonicalize().map_err(|e| {
                medical_core::error::AppError::Other(format!("path canonicalize failed: {e}"))
            })?;
            let canonical_dir = recordings_dir.canonicalize().map_err(|e| {
                medical_core::error::AppError::Other(format!(
                    "recordings dir canonicalize failed: {e}"
                ))
            })?;
            if !canonical_path.starts_with(&canonical_dir) {
                tracing::warn!(
                    id_len,
                    "content_audio: path outside recordings dir — rejected"
                );
                return Err(medical_core::error::AppError::Other(
                    "audio path not allowed".into(),
                ));
            }
            match file_crypto::decrypt_file(path) {
                Ok(plaintext) => Ok(plaintext),
                Err(file_crypto::FileCryptoError::NotEncrypted) => {
                    // Legacy plaintext file — read as-is.
                    std::fs::read(path).map_err(|e| {
                        medical_core::error::AppError::Other(format!("audio read failed: {e}"))
                    })
                }
                Err(e) => Err(medical_core::error::AppError::security(format!(
                    "audio decrypt failed: {e}"
                ))),
            }
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            warn!(id_len, error = %e, "content_audio: get failed");
            if matches!(
                e,
                medical_core::error::AppError::Other(ref s)
                    if s.contains("not found")
            ) {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    let byte_count = bytes.len();
    info!(id_len, byte_count, "content_audio: get");
    Ok(bytes.into_response())
}

/// PUT /v1/content/audio/{recording_id} — upload audio bytes (first-write-wins).
///
/// Receives the raw audio bytes in the request body, writes them to a temp
/// file under the recordings dir, encrypts in place (atomic rename), then
/// updates the recording's `audio_path`. If a file already exists for this
/// recording, returns 409 Conflict (first-write-wins). Returns 201 Created
/// on success. Only the ID length and byte count are logged.
pub(super) async fn content_audio_put_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(recording_id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let id_len = recording_id.len();
    let byte_count = body.len();

    // Validate recording exists before writing anything to disk.
    let db = Arc::clone(&state.db);
    let uuid = Uuid::parse_str(&recording_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let exists = tokio::task::spawn_blocking({
        let db = Arc::clone(&db);
        move || -> Result<bool, StatusCode> {
            let conn = db.conn().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            match RecordingsRepo::get_by_id(&conn, &uuid) {
                Ok(_) => Ok(true),
                Err(medical_db::DbError::NotFound(_)) => Ok(false),
                Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;
    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let data_dir = state.data_dir.clone();
    let target_path = {
        // Resolve the recordings dir synchronously (cheap: reads settings).
        let recordings_dir =
            crate::commands::resolve_recordings_dir(&db, &data_dir).map_err(|e| {
                warn!(id_len, error = %e, "content_audio: resolve dir failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        recordings_dir.join(format!("{recording_id}.enc"))
    };

    // First-write-wins: if the target already exists, refuse.
    if target_path.exists() {
        info!(id_len, "content_audio: put rejected, file exists (409)");
        return Err(StatusCode::CONFLICT);
    }

    // Encrypt in memory first, then write ciphertext directly to a unique temp
    // file and atomically rename. This avoids ever writing plaintext PHI to
    // disk — critical for HIPAA at-rest guarantees (H9 fix).
    let db2 = Arc::clone(&db);
    let path_for_db = target_path.clone();
    let body_vec = body.to_vec();
    tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        // Encrypt the plaintext bytes in memory (no disk I/O).
        let ciphertext = file_crypto::encrypt_bytes_in_memory(&body_vec).map_err(|e| {
            medical_core::error::AppError::security(format!("audio encrypt failed: {e}"))
        })?;

        // Write ciphertext to a unique temp file, then atomic rename.
        let tmp_path = target_path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp_path, &ciphertext)?;
        if let Err(e) = std::fs::rename(&tmp_path, &target_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(medical_core::error::AppError::Io(e));
        }

        // Update audio_path on the recording row.
        let conn = db2.conn()?;
        let mut rec =
            RecordingsRepo::get_by_id(&conn, &uuid).map_err(medical_core::error::AppError::from)?;
        rec.audio_path = path_for_db;
        rec.file_size_bytes = Some(body_vec.len() as u64);
        RecordingsRepo::update(&conn, &rec).map_err(medical_core::error::AppError::from)?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!(id_len, error = %e, "content_audio: put failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Notify other clients (SSE) and THIS server's own webview that the
    // recording's audio landed. Recordings sync in two phases (metadata,
    // then audio); without this emit the server UI's audio state stays
    // stale after phase 2 until the next background refresh. Mirrors the
    // metadata push handler's notify pattern.
    let _ = state.content_changed_tx.send(recording_id.clone());
    let _ = state.app_handle.emit(
        "recording-updated",
        serde_json::json!({ "id": recording_id }),
    );

    info!(id_len, byte_count, "content_audio: put (201)");
    Ok(StatusCode::CREATED)
}
