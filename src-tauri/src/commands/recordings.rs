use std::path::PathBuf;

use medical_core::error::{AppError, AppResult};
use medical_core::types::recording::{ProcessingStatus, Recording, RecordingSummary};
use medical_db::recordings::RecordingsRepo;
use medical_db::search::SearchRepo;
use medical_db::vectors::VectorsRepo;
use uuid::Uuid;

use super::{join_err, resolve_recordings_dir};
use crate::state::AppState;

/// List recordings with optional pagination.
///
/// Returns up to `limit` (default 50) recordings starting at `offset` (default 0),
/// ordered by creation date descending.
#[tauri::command]
pub async fn list_recordings(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<RecordingSummary>> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::list_all(&conn, limit.unwrap_or(50), offset.unwrap_or(0))
            .map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Get a single recording by its UUID.
///
/// Returns the full `Recording` including transcript, SOAP note, and metadata.
#[tauri::command]
pub async fn get_recording(state: tauri::State<'_, AppState>, id: String) -> AppResult<Recording> {
    let uuid =
        Uuid::parse_str(&id).map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Full-text search across recording transcripts and SOAP notes.
///
/// Returns up to `limit` (default 20) matching recordings.
#[tauri::command]
pub async fn search_recordings(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> AppResult<Vec<Recording>> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        SearchRepo::search_recordings(&conn, &query, limit.unwrap_or(20)).map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Delete a recording by UUID.
///
/// Removes the DB row, associated RAG vectors, and the WAV file from disk.
/// The DB delete and vector cleanup are atomic (same transaction); the WAV
/// Soft-delete a recording. Marks `deleted_at` on the row and removes it
/// from FTS. The WAV file and RAG vectors are **preserved** for undo. A
/// future purge sweeper will permanently delete old soft-deleted recordings.
///
/// The frontend shows an Undo toast for 8 seconds after this succeeds.
#[tauri::command]
pub async fn delete_recording(state: tauri::State<'_, AppState>, id: String) -> AppResult<()> {
    let uuid =
        Uuid::parse_str(&id).map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let db = state.db.clone();
    let db_for_push = state.db.clone();
    let http = state.http_client.clone();

    // Soft-delete and the sync-target gates are both blocking (SQLite pool
    // checkout, config load, OS keychain read) — run them on the blocking
    // pool, never the async runtime (and this command used to be sync, i.e.
    // all of it ran on the main thread).
    let sync_target = tokio::task::spawn_blocking(
        move || -> AppResult<
            Option<(
                crate::commands::sharing::PairedConnection,
                String,
                std::sync::Arc<reqwest::Client>,
            )>,
        > {
            let conn = db.conn()?;
            // Soft-delete: mark the row as deleted. NotFound is a no-op success
            // (the user's intent is to delete, so an already-absent row is fine).
            match RecordingsRepo::soft_delete(&conn, &uuid) {
                Ok(()) => {}
                Err(medical_db::DbError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
            Ok(crate::commands::content_sync::content_sync_target_parts(
                &db_for_push,
                http,
            ))
        },
    )
    .await
    .map_err(join_err)??;

    // Best-effort content sync push of the tombstone. Resolve the sync target
    // (owned PairedConnection + bearer + client) and the db clone here, then
    // move them into a fire-and-forget task — `tauri::State` is a borrow and
    // can't cross the spawn boundary. Mirrors the condition-chip push pattern.
    if let Some((conn_paired, bearer, client)) = sync_target {
        let db = state.db.clone();
        let rec_id = id.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(remote) =
                crate::content_remote::ContentRemote::from(&conn_paired, Some(bearer), client)
            {
                let result = tokio::task::spawn_blocking(move || -> AppResult<Vec<_>> {
                    let c = db.conn()?;
                    let mut sync_rec =
                        crate::commands::content_sync::build_sync_recording(&c, &rec_id)?;
                    // Read deleted_at to include the tombstone marker so the
                    // server's merge sees the deletion.
                    let deleted_at: Option<String> = c
                        .query_row(
                            "SELECT deleted_at FROM recordings WHERE id = ?1",
                            rusqlite::params![rec_id],
                            |row| row.get(0),
                        )
                        .ok()
                        .flatten();
                    sync_rec.deleted_at = deleted_at;
                    Ok(vec![sync_rec])
                })
                .await;
                if let Ok(Ok(recordings)) = result {
                    let _ = remote.push(recordings).await;
                }
            }
        });
    }

    Ok(())
}

/// Restore a soft-deleted recording (undo). Clears `deleted_at` and
/// re-inserts the FTS row so search finds it again.
#[tauri::command]
pub async fn restore_recording(state: tauri::State<'_, AppState>, id: String) -> AppResult<()> {
    let uuid =
        Uuid::parse_str(&id).map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn()?;
        RecordingsRepo::restore(&conn, &uuid)?;
        Ok(())
    })
    .await
    .map_err(join_err)?
}

/// Delete RAG vectors for a recording, logging failures rather than aborting
/// the recording deletion. Used by the future purge sweeper (permanent delete).
/// The soft-delete path preserves vectors for undo.
#[allow(dead_code)]
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
///
/// Also resets the content-sync cursors so the next sync re-pulls
/// everything from the partner machine. Without this, Delete All would
/// permanently break sync — the partner's push cursor would already be
/// advanced past its old recordings, and the local pull cursor would skip
/// the partner's recordings that were already "seen" (but now deleted
/// locally).
#[tauri::command]
pub async fn delete_all_recordings(state: tauri::State<'_, AppState>) -> AppResult<u32> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> AppResult<u32> {
        let conn = db.conn()?;

        // Wrap deletes + cursor resets in a single transaction so a crash
        // between them can't leave diverged cursors.
        conn.execute_batch("BEGIN")
            .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
        let result: AppResult<(Vec<std::path::PathBuf>, u32)> = (|| {
            // Delete all RAG vectors
            if let Err(e) = conn.execute("DELETE FROM vectors", []) {
                tracing::error!(error = %e, "RAG vector cleanup failed during delete_all_recordings; orphan vectors may remain");
            }

            // Delete all recordings and get audio paths for file cleanup
            let paths = RecordingsRepo::delete_all(&conn).map_err(AppError::from)?;
            let count = paths.len() as u32;

            // Reset content-sync cursors so the next sync re-syncs everything.
            if let Err(e) = medical_db::content_sync::ContentSyncRepo::set_cursor(&conn, None) {
                tracing::warn!(error = %e, "failed to reset pull cursor after Delete All");
            }
            if let Err(e) = conn.execute(
                "UPDATE sync_state SET value = NULL WHERE key = 'content_sync_push_cursor'",
                [],
            ) {
                tracing::warn!(error = %e, "failed to reset push cursor after Delete All");
            }
            tracing::info!("Delete All: content-sync cursors reset, next sync will re-pull everything");

            Ok((paths, count))
        })();

        let (paths, count) = match result {
            Ok(v) => {
                conn.execute_batch("COMMIT")
                    .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
                v
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        };

        // Remove audio files from disk (after commit, so we only delete
        // if the DB transaction succeeded).
        for path in &paths {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }

        Ok(count)
    })
    .await
    .map_err(join_err)?
}

/// Import an audio file from the filesystem into the recordings library.
///
/// Non-WAV files (MP3, FLAC, OGG, M4A, AAC) are automatically converted to
/// WAV so the transcription pipeline can process them.  Creates a Recording
/// entry in the database and returns the new recording ID.
#[tauri::command]
pub async fn import_audio_file(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> AppResult<String> {
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    // The whole import — dir resolution (settings read), file copy or
    // in-process decode/convert, WAV parse, and at-rest encryption — is
    // blocking and can take seconds on a large file.
    tokio::task::spawn_blocking(move || -> AppResult<String> {
        let source = PathBuf::from(&file_path);
        if !source.exists() {
            return Err(AppError::Other(format!("File not found: {file_path}")));
        }

        // Resolve recordings directory from settings (custom path or default).
        let recordings_dir = resolve_recordings_dir(&db, &data_dir)?;

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
                .map_err(|e| AppError::audio(format!("Failed to copy file: {e}")))?;
            dest
        } else {
            // Non-WAV — convert to WAV.
            let dest_filename = format!("{original_name}_{short_id}.wav");
            let dest = recordings_dir.join(&dest_filename);
            medical_audio::convert::convert_to_wav(&source, &dest)
                .map_err(|e| AppError::audio(format!("Failed to convert audio: {e}")))?;
            dest
        };

        // Read duration and file size from the resulting WAV. If the just-written
        // WAV is unreadable, that's a real signal (corrupt source, converter bug)
        // — surface it instead of silently setting duration=None.
        let file_size = std::fs::metadata(&dest_path)
            .map(|m| m.len())
            .map_err(|e| AppError::audio(format!("imported WAV unreadable: {e}")))?;
        let duration = {
            let reader = hound::WavReader::open(&dest_path)
                .map_err(|e| AppError::audio(format!("imported WAV unreadable: {e}")))?;
            let spec = reader.spec();
            let total_samples = reader.len() as f64;
            if spec.sample_rate > 0 && spec.channels > 0 {
                total_samples / (spec.sample_rate as f64 * spec.channels as f64)
            } else {
                0.0
            }
        };

        // Encrypt the imported recording at rest (same as captured recordings).
        // Best-effort: a keychain failure falls back to plaintext so the import
        // isn't lost, matching the capture path's behavior.
        if file_size > 0
            && let Err(e) = medical_security::file_crypto::encrypt_file_in_place(&dest_path)
        {
            use medical_security::file_crypto::FileCryptoError;
            match e {
                FileCryptoError::Keychain(e) => {
                    tracing::warn!(error = %e, "import: could not encrypt (keychain unavailable); storing plaintext")
                }
                e => {
                    tracing::warn!(error = %e, path = %dest_path.display(), "import: could not encrypt; storing plaintext")
                }
            }
        }

        let dest_filename = dest_path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{original_name}_{short_id}.wav"));

        // Create the Recording entry.
        let mut recording = Recording::new(dest_filename, dest_path);
        recording.id = recording_id;
        recording.duration_seconds = Some(duration);
        recording.file_size_bytes = Some(file_size);
        recording.status = ProcessingStatus::Pending;

        let conn = db.conn()?;
        RecordingsRepo::insert(&conn, &recording)?;

        Ok(recording_id.to_string())
    })
    .await
    .map_err(join_err)?
}
