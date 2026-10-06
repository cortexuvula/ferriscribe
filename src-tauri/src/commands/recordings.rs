use std::path::PathBuf;
use std::sync::Arc;

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

/// Move a recording to Trash (single delete).
///
/// Soft-deletes: marks `deleted_at` on the row and removes it from FTS in
/// one transaction. The WAV file and RAG vectors are **preserved** for the
/// 30-day recovery window; the purge sweeper permanently deletes trashed
/// recordings after 30 days. The tombstone is pushed to the paired server
/// (fire-and-forget, backstopped by the periodic sync).
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
    if let Some(parts) = sync_target {
        spawn_recordings_push(Some(parts), state.db.clone(), vec![id]);
    }

    Ok(())
}

/// Fire-and-forget push of the CURRENT wire state for the given recording
/// ids to the paired server. Serves BOTH directions: after a soft-delete the
/// rows carry `deleted_at` (tombstone), after a restore they carry
/// `deleted_at = NULL` (revive) — `build_sync_recording` reads the marker
/// from the row, so the same helper covers delete and restore pushes.
///
/// A failure here is acceptable ONLY because the periodic sync backstops it:
/// its `changed_since` selection filters by `updated_at` with NO
/// `deleted_at` filter, and both soft-delete and restore bump `updated_at`,
/// so tombstones and revives re-travel on the next cycle regardless. NEVER
/// add a `deleted_at IS NULL` filter to the periodic push path — that would
/// silently break deletion propagation in the privacy-critical direction.
///
/// Logs carry counts and ids only — never recording content (PHI).
fn spawn_recordings_push(parts: SyncPushParts, db: Arc<medical_db::Database>, ids: Vec<String>) {
    let Some((conn_paired, bearer, client)) = parts else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let Some(remote) =
            crate::content_remote::ContentRemote::from(&conn_paired, Some(bearer), client)
        else {
            return;
        };
        let result = tokio::task::spawn_blocking(move || -> AppResult<Vec<_>> {
            let c = db.conn()?;
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match crate::commands::content_sync::build_sync_recording(&c, id) {
                    Ok(sync_rec) => out.push(sync_rec),
                    Err(e) => tracing::warn!(
                        recording_id_len = id.len(),
                        error = %e,
                        "recordings push: skipping unreadable recording"
                    ),
                }
            }
            Ok(out)
        })
        .await;
        match result {
            Ok(Ok(recordings)) if !recordings.is_empty() => {
                let count = recordings.len();
                if let Err(e) = remote.push(recordings).await {
                    tracing::warn!(
                        error = %e,
                        count,
                        "recordings push failed (fire-and-forget; periodic sync will retry)"
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "recordings push: build task failed"),
        }
    });
}

/// Restore a soft-deleted recording (undo). Clears `deleted_at` and
/// re-inserts the FTS row so search finds it again.
///
/// The revive is pushed to the paired server (fire-and-forget): a sync
/// recording with `deleted_at = null`, which the server's restore-vs-
/// tombstone LWW merge resolves. The periodic sync backstops a failed
/// push (restore bumps `updated_at`, so the row re-travels).
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
    .map_err(join_err)??;

    // Revive push — same fire-and-forget shape as the delete push; the
    // row now carries deleted_at = NULL, which is the revive marker.
    let parts = crate::commands::content_sync::content_sync_target(&state).await;
    spawn_recordings_push(parts, state.db.clone(), vec![id]);
    Ok(())
}

/// Result of any bulk restore path: the ACTUAL count restored plus the
/// restored ids. The count can be lower than a preview (D6) when a purge
/// or a concurrent restore changed the candidate set in between — callers
/// report actual vs preview, never assume equality.
#[derive(serde::Serialize)]
pub struct RestoreResult {
    pub count: u32,
    pub ids: Vec<String>,
}

/// Restore multiple soft-deleted recordings by id — the exact-set batch
/// Undo for Move-all-to-Trash, and the shared backend for bulk restore
/// surfaces. One transaction (`restore_many`); ids that are not currently
/// trashed are skipped, not errors. The revived rows are pushed to the
/// paired server (fire-and-forget, backstopped by the periodic sync).
#[tauri::command]
pub async fn restore_recordings(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> AppResult<RestoreResult> {
    let mut uuids = Vec::with_capacity(ids.len());
    for id in &ids {
        let uuid = Uuid::parse_str(id)
            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
        uuids.push(uuid);
    }
    let db = state.db.clone();
    let restored = tokio::task::spawn_blocking(move || -> AppResult<Vec<Uuid>> {
        let conn = db.conn()?;
        RecordingsRepo::restore_many(&conn, &uuids).map_err(AppError::from)
    })
    .await
    .map_err(join_err)??;

    let count = restored.len() as u32;
    let id_strs: Vec<String> = restored.iter().map(|i| i.to_string()).collect();
    let parts = crate::commands::content_sync::content_sync_target(&state).await;
    spawn_recordings_push(parts, state.db.clone(), id_strs.clone());
    tracing::info!(count, "restored recordings from Trash (batch undo)");
    Ok(RestoreResult {
        count,
        ids: id_strs,
    })
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

/// Result of "Move all to Trash": the count plus the exact id set trashed.
/// The Undo toast restores exactly these ids — a recording deleted AFTER
/// the move must never be swept into that undo (which a "restore everything
/// deleted since T" design would do).
#[derive(serde::Serialize)]
pub struct DeleteAllResult {
    pub count: u32,
    pub ids: Vec<String>,
}

/// The paired-server push target once resolved (owned connection + bearer
/// + HTTP client), as produced by `content_sync_target_parts`.
pub(crate) type SyncPushParts = Option<(
    crate::commands::sharing::PairedConnection,
    String,
    Arc<reqwest::Client>,
)>;

/// Blocking outcome of Move-all-to-Trash: the trashed ids plus the
/// resolved push target for the fire-and-forget tombstone batch.
type SoftDeleteAllOutcome = (Vec<Uuid>, SyncPushParts);

/// Move every active recording to Trash (reversible Delete All).
///
/// Soft-deletes all visible rows in one transaction (`soft_delete_all`):
/// rows tombstoned, FTS de-indexed, WAV files and RAG vectors preserved
/// for the 30-day recovery window. Content-sync cursors are NOT reset —
/// tombstones are the propagation mechanism (a cursor reset would
/// resurrect everything from a partner on the next pull; the old
/// hard-delete rationale no longer applies).
///
/// Returns the trashed count and exact id set; after commit, the tombstones
/// are pushed to the paired server in one fire-and-forget batch.
#[tauri::command]
pub async fn delete_all_recordings(
    state: tauri::State<'_, AppState>,
) -> AppResult<DeleteAllResult> {
    let db = state.db.clone();
    let db_for_push = state.db.clone();
    let http = state.http_client.clone();

    // The soft-delete transaction and the sync-target gates (SQLite pool
    // checkout, config load, OS keychain read) are both blocking — one
    // blocking hop for both, mirroring `delete_recording`.
    let (ids, parts) = tokio::task::spawn_blocking(move || -> AppResult<SoftDeleteAllOutcome> {
        let conn = db.conn()?;
        let ids = RecordingsRepo::soft_delete_all(&conn).map_err(AppError::from)?;
        let parts = crate::commands::content_sync::content_sync_target_parts(&db_for_push, http);
        Ok((ids, parts))
    })
    .await
    .map_err(join_err)??;

    let count = ids.len() as u32;
    let id_strs: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    spawn_recordings_push(parts, state.db.clone(), id_strs.clone());
    tracing::info!(
        count,
        "Move all to Trash: recordings moved (30-day recovery window)"
    );
    Ok(DeleteAllResult {
        count,
        ids: id_strs,
    })
}

/// Authoritative count of active (non-trashed) recordings.
///
/// The frontend's `recordings.list` is a paginated subset — dialogs that
/// promise "all N recordings" (Move-all-to-Trash confirm) must use this
/// number instead.
#[tauri::command]
pub async fn count_recordings(state: tauri::State<'_, AppState>) -> AppResult<u32> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::count(&conn).map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// One page of the Trash view plus the authoritative total (D4).
#[derive(serde::Serialize)]
pub struct TrashedListResult {
    pub items: Vec<medical_core::types::recording::TrashedRecordingSummary>,
    pub total: u32,
}

/// List soft-deleted recordings (Trash view), newest deletion first.
/// Summaries carry structural metadata only — never transcript/SOAP
/// content.
#[tauri::command]
pub async fn list_trashed_recordings(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<TrashedListResult> {
    let db = state.db.clone();
    let (items, total) = tokio::task::spawn_blocking(move || -> AppResult<(_, _)> {
        let conn = db.conn()?;
        RecordingsRepo::list_trashed(&conn, limit.unwrap_or(50), offset.unwrap_or(0))
            .map_err(AppError::from)
    })
    .await
    .map_err(join_err)??;
    Ok(TrashedListResult { items, total })
}

/// Validate a `[start, end)` RFC3339 pair for the day-scoped restore
/// surfaces (D6): both must parse, and start must be strictly before end.
/// Shared by the count-preview and the restore command so the two can
/// never disagree about what a valid window is.
fn validate_interval(
    start_iso: &str,
    end_iso: &str,
) -> AppResult<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let parse = |s: &str, label: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| AppError::InvalidInput(format!("invalid {label} timestamp: {e}")))
    };
    let start = parse(start_iso, "start")?;
    let end = parse(end_iso, "end")?;
    if start >= end {
        return Err(AppError::InvalidInput(
            "start timestamp must be before end timestamp".into(),
        ));
    }
    Ok((start, end))
}

/// Restore EVERY recording in Trash ("Restore all", D5). Returns the
/// actual restored count; revives are pushed to the paired server.
#[tauri::command]
pub async fn restore_all_trashed(state: tauri::State<'_, AppState>) -> AppResult<RestoreResult> {
    let db = state.db.clone();
    let restored = tokio::task::spawn_blocking(move || -> AppResult<Vec<Uuid>> {
        let conn = db.conn()?;
        RecordingsRepo::restore_all(&conn).map_err(AppError::from)
    })
    .await
    .map_err(join_err)??;

    let count = restored.len() as u32;
    let id_strs: Vec<String> = restored.iter().map(|i| i.to_string()).collect();
    let parts = crate::commands::content_sync::content_sync_target(&state).await;
    spawn_recordings_push(parts, state.db.clone(), id_strs.clone());
    tracing::info!(count, "restored all recordings from Trash");
    Ok(RestoreResult {
        count,
        ids: id_strs,
    })
}

/// Preview count for restore-by-date (D6): how many recordings currently
/// in Trash were moved there inside `[start_iso, end_iso)`. MUST be the
/// count command — never derived from paginated trash pages.
#[tauri::command]
pub async fn count_recordings_deleted_between(
    state: tauri::State<'_, AppState>,
    start_iso: String,
    end_iso: String,
) -> AppResult<u32> {
    validate_interval(&start_iso, &end_iso)?;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::count_deleted_between(&conn, &start_iso, &end_iso).map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Restore every recording moved to Trash inside the half-open
/// `[start_iso, end_iso)` window (exact local-calendar-day restore, D6).
/// Returns the ACTUAL restored count — a purge or concurrent restore may
/// have changed the candidate set since the preview; callers report
/// actual vs preview.
#[tauri::command]
pub async fn restore_recordings_deleted_between(
    state: tauri::State<'_, AppState>,
    start_iso: String,
    end_iso: String,
) -> AppResult<RestoreResult> {
    validate_interval(&start_iso, &end_iso)?;
    let db = state.db.clone();
    let restored = tokio::task::spawn_blocking(move || -> AppResult<Vec<Uuid>> {
        let conn = db.conn()?;
        RecordingsRepo::restore_deleted_between(&conn, &start_iso, &end_iso).map_err(AppError::from)
    })
    .await
    .map_err(join_err)??;

    let count = restored.len() as u32;
    let id_strs: Vec<String> = restored.iter().map(|i| i.to_string()).collect();
    let parts = crate::commands::content_sync::content_sync_target(&state).await;
    spawn_recordings_push(parts, state.db.clone(), id_strs.clone());
    tracing::info!(count, "restored recordings from Trash by deletion date");
    Ok(RestoreResult {
        count,
        ids: id_strs,
    })
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
        // The row is inserted (with `encryption_pending = 1`) BEFORE the
        // encrypt attempt, in one transaction — mirroring stop_recording's
        // ordering. A crash or a transient keychain/IO failure mid-encrypt
        // then leaves a flagged plaintext WAV the boot sweep retries, instead
        // of an invisible orphan outside the sweep's view.
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
        let should_encrypt = file_size > 0;
        if should_encrypt {
            conn.execute_batch("BEGIN")
                .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
            let result: medical_db::DbResult<()> = (|| {
                RecordingsRepo::insert(&conn, &recording)?;
                conn.execute(
                    "UPDATE recordings SET encryption_pending = 1 WHERE id = ?1",
                    [&recording_id.to_string()],
                )
                .map_err(medical_db::DbError::from)?;
                Ok(())
            })();
            match result {
                Ok(()) => conn
                    .execute_batch("COMMIT")
                    .map_err(|e| AppError::from(medical_db::DbError::from(e)))?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(AppError::from(e));
                }
            }
        } else {
            RecordingsRepo::insert(&conn, &recording)?;
        }

        if should_encrypt {
            match medical_security::file_crypto::encrypt_file_in_place(&recording.audio_path) {
                Ok(()) => {
                    RecordingsRepo::set_encryption_done(&conn, &recording_id)?;
                }
                Err(e) => {
                    // Leave the flag set — the boot sweep retries, matching
                    // the capture path's failure semantics.
                    use medical_security::file_crypto::FileCryptoError;
                    match e {
                        FileCryptoError::Keychain(e) => {
                            tracing::warn!(error = %e, "import: could not encrypt (keychain unavailable); encryption_pending stays set for the boot sweep")
                        }
                        e => {
                            tracing::warn!(error = %e, path = %recording.audio_path.display(), "import: could not encrypt; encryption_pending stays set for the boot sweep")
                        }
                    }
                }
            }
        }

        Ok(recording_id.to_string())
    })
    .await
    .map_err(join_err)?
}
