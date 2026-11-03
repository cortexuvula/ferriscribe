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

/// All active recording creation instants, one per recording (duplicates
/// retained — the browser groups them into local-date counts).
///
/// The availability payload is timestamps ONLY: no ids, names, filenames,
/// paths, transcripts, or metadata (PHI minimization — the calendar needs
/// nothing else). Grouping into local dates happens in the browser's own
/// timezone; the backend carries no timezone dependency. A corrupt stored
/// timestamp surfaces as an error here rather than a silently empty
/// calendar.
#[tauri::command]
pub async fn list_recording_creation_times(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<chrono::DateTime<chrono::Utc>>> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::list_creation_times(&conn).map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Limit handling for [`query_recordings`]: default 50, capped at 200 — an
/// absurd caller request must never fetch the whole table in one page.
fn query_limit_or_default(limit: Option<u32>) -> u32 {
    limit.unwrap_or(50).min(200)
}

/// Normalize the optional `[start, end)` creation-window pair for
/// [`query_recordings`]: both bounds together or neither (an incomplete
/// pair is rejected — never an unfiltered fallback), both must parse and
/// the window must not be reversed (reusing `validate_interval`), and the
/// result is UTC-normalized RFC 3339 so the SQL compares like-for-like
/// against stored timestamps of either format.
fn query_interval_bounds(
    start_iso: Option<&str>,
    end_iso: Option<&str>,
) -> AppResult<Option<(String, String)>> {
    match (start_iso, end_iso) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => {
            let (start, end) = validate_interval(start, end)?;
            Ok(Some((start.to_rfc3339(), end.to_rfc3339())))
        }
        _ => Err(AppError::InvalidInput(
            "start and end timestamps must be provided together".into(),
        )),
    }
}

/// Composite query for the Active recordings list (recording-date-calendar):
/// optional literal-term FTS `search`, optional half-open
/// `[startIso, endIso)` creation window, and paging.
///
/// All filters are applied in SQL before LIMIT/OFFSET, results come back in
/// deterministic actual-instant newest-first order (id as tie-breaker), and
/// the projection is summary flags — never transcript/SOAP documents. See
/// `RecordingsRepo::query` for the normalization and search-escaping
/// details. Whitespace-only `search` means "no search filter" (the caller's
/// search box being empty must not zero out the list); a search whose terms
/// all strip away (quotes-only) matches nothing, mirroring
/// `search_recordings`.
#[tauri::command]
pub async fn query_recordings(
    state: tauri::State<'_, AppState>,
    search: Option<String>,
    start_iso: Option<String>,
    end_iso: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<RecordingSummary>> {
    let search = search
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let interval = query_interval_bounds(start_iso.as_deref(), end_iso.as_deref())?;
    let limit = query_limit_or_default(limit);
    let offset = offset.unwrap_or(0);

    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::query(
            &conn,
            search.as_deref(),
            interval
                .as_ref()
                .map(|(start, end)| (start.as_str(), end.as_str())),
            limit,
            offset,
        )
        .map_err(AppError::from)
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

/// One "Purge Now" candidate on the wire: the recording id plus the RAW
/// stored deletion timestamp from the preview. camelCase on the wire to
/// match the frontend `TrashPurgeCandidate`. The identity travels
/// byte-for-byte — the backend purge re-verifies it inside the deletion
/// transaction with raw-string equality.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashPurgeCandidateDto {
    pub id: String,
    pub deleted_at: String,
}

/// Backend preview for the "Purge Now" confirmation: the authoritative,
/// all-DB candidate set (any age, independent of pages/search filters)
/// plus the count of trashed rows still mid-encryption (never purged —
/// the dialog says they will be kept). Ids and timestamps only, no PHI.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashPurgePreview {
    pub candidates: Vec<TrashPurgeCandidateDto>,
    pub protected_count: u32,
}

/// Result of the destructive "Purge Now" purge: ACTUAL counts, never
/// fabricated — `purged` rows are gone, `protected` were kept
/// (mid-encryption; retry later), `changed` left the reviewed set (a new
/// preview + confirmation is needed), `cleanup_pending` means some audio
/// removals did not complete in this call but have DURABLE COMMITTED
/// cleanup intent — the boot/tick drain will finish them (crash, unlink
/// failure, or ack-write failure all recover).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashPurgeResult {
    pub purged: u32,
    pub protected: u32,
    pub changed: u32,
    pub cleanup_pending: bool,
}

/// Validate + convert the wire candidates for [`purge_trash_now`]: a
/// malformed id or a blank tombstone identity is rejected BEFORE any DB
/// work — never silently dropped (the purge would under-deliver against
/// its confirmation) and never defaulted (a fabricated identity could
/// purge an unreviewed deletion).
fn parse_trash_purge_candidates(
    dtos: Vec<TrashPurgeCandidateDto>,
) -> AppResult<Vec<medical_db::recordings::TrashPurgeSnapshotCandidate>> {
    dtos.into_iter()
        .map(|dto| {
            let id = Uuid::parse_str(&dto.id)
                .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
            if dto.deleted_at.trim().is_empty() {
                return Err(AppError::InvalidInput(
                    "purge candidate is missing its deletion timestamp".into(),
                ));
            }
            Ok(medical_db::recordings::TrashPurgeSnapshotCandidate {
                id,
                deleted_at: dto.deleted_at,
            })
        })
        .collect()
}

/// Preview the exact-set "Purge Now" candidates: every trashed recording
/// regardless of age (fresh and future-dated tombstones included),
/// excluding rows still mid-encryption. The confirmation dialog must show
/// `candidates.len()` — never the paginated list length or the badge.
#[tauri::command]
pub async fn preview_trash_purge(
    state: tauri::State<'_, AppState>,
) -> AppResult<TrashPurgePreview> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> AppResult<TrashPurgePreview> {
        let conn = db.conn()?;
        let (candidates, protected_count) =
            RecordingsRepo::list_soft_deleted_snapshot(&conn).map_err(AppError::from)?;
        Ok(TrashPurgePreview {
            candidates: candidates
                .into_iter()
                .map(|c| TrashPurgeCandidateDto {
                    id: c.id.to_string(),
                    deleted_at: c.deleted_at,
                })
                .collect(),
            protected_count,
        })
    })
    .await
    .map_err(join_err)?
}

/// PHI-safe failure diagnostic for the manual "Purge Now" command: static
/// operation name, the stable `AppError` kind discriminant, and the numeric
/// SQLite extended code when the source chain carries one. NEVER the raw
/// Display/Debug/`message`/`source`/serialized `AppError` — `Database`'s
/// message is the `DbError` Display, which can echo SQL and row data (PHI),
/// and the wrapped `rusqlite` error message can echo engine strings with
/// bound values.
fn log_purge_failure_diagnostic(err: &AppError) {
    tracing::warn!(
        operation = "purge_trash_now",
        error_kind = err.kind_str(),
        sqlite_extended_code = sqlite_extended_code(err).unwrap_or(0),
        "Purge Now failed"
    );
}

/// Numeric SQLite extended result code buried in `err`'s source chain, if
/// any (e.g. 267 = SQLITE_CORRUPT_VTAB). Pure number — no engine text.
fn sqlite_extended_code(err: &AppError) -> Option<i32> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(rusqlite::Error::SqliteFailure(ffi, _)) = e.downcast_ref::<rusqlite::Error>() {
            return Some(ffi.extended_code);
        }
        current = e.source();
    }
    None
}

/// Permanently purge EXACTLY the reviewed candidates ("Purge Now"). The
/// frozen preview set travels verbatim; the backend re-verifies each
/// tombstone identity plus the mid-encryption guard inside ONE deletion
/// transaction (purge-first artifact cleanup follows only for confirmed
/// ids — see `sweeps::purge_trash_snapshot`). No paired-server push: the
/// tombstones already travelled at soft-delete time, and the
/// `purged_recordings` ledger written in the same transaction blocks
/// stale peer copies from resurrecting the rows.
#[tauri::command]
pub async fn purge_trash_now(
    state: tauri::State<'_, AppState>,
    candidates: Vec<TrashPurgeCandidateDto>,
) -> AppResult<TrashPurgeResult> {
    let snapshot = parse_trash_purge_candidates(candidates)?;
    let db = state.db.clone();
    let outcome =
        tokio::task::spawn_blocking(move || crate::sweeps::purge_trash_snapshot(&db, &snapshot))
            .await
            .map_err(join_err)?
            // Scoped PHI-safe diagnostic (v0.85.0 field report: purge failures
            // left NO log line — only the success path logged). Structured
            // statics only — see `log_purge_failure_diagnostic`; the frontend
            // toast stays sanitized (sanitizedErr).
            .inspect_err(log_purge_failure_diagnostic)?;
    tracing::info!(
        purged = outcome.purged.len(),
        protected = outcome.protected.len(),
        changed = outcome.changed.len(),
        cleanup_pending = outcome.cleanup_pending,
        "Purge Now: manual Trash purge completed"
    );
    Ok(TrashPurgeResult {
        purged: outcome.purged.len() as u32,
        protected: outcome.protected.len() as u32,
        changed: outcome.changed.len() as u32,
        cleanup_pending: outcome.cleanup_pending,
    })
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

/// Preview permanent-deletion eligibility before shortening the Trash window.
/// The persistence boundary rechecks this count atomically when saving.
#[tauri::command]
pub async fn count_soft_deleted_older_than(
    state: tauri::State<'_, AppState>,
    days: u32,
) -> AppResult<u32> {
    if !medical_core::types::settings::TRASH_RETENTION_DAYS_CHOICES.contains(&days) {
        return Err(AppError::InvalidInput(
            "Trash window must be 2, 7, 14, 21, or 30 days.".into(),
        ));
    }
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::count_soft_deleted_older_than(&conn, days, chrono::Utc::now())
            .map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
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

/// Preview count for move-by-recorded-date-range: how many ACTIVE
/// recordings were created inside `[start_iso, end_iso)`. MUST be the
/// count command — never derived from the paginated Active list, which is
/// filtered and paged.
#[tauri::command]
pub async fn count_recordings_created_between(
    state: tauri::State<'_, AppState>,
    start_iso: String,
    end_iso: String,
) -> AppResult<u32> {
    validate_interval(&start_iso, &end_iso)?;
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        RecordingsRepo::count_active_created_between(&conn, &start_iso, &end_iso)
            .map_err(AppError::from)
    })
    .await
    .map_err(join_err)?
}

/// Move every ACTIVE recording created inside the half-open
/// `[start_iso, end_iso)` window to Trash ("Move by date range", recorded
/// date, both ends inclusive in the UI). Mirrors `delete_all_recordings`:
/// one blocking hop for the soft-delete transaction plus the sync-target
/// gates, then a fire-and-forget tombstone batch push. Returns the
/// ACTUAL moved count and exact id set — a concurrent deletion may have
/// shrunk the candidate set since the preview; callers report actual vs
/// preview, and the Undo restores exactly these ids.
#[tauri::command]
pub async fn delete_recordings_created_between(
    state: tauri::State<'_, AppState>,
    start_iso: String,
    end_iso: String,
) -> AppResult<DeleteAllResult> {
    validate_interval(&start_iso, &end_iso)?;
    let db = state.db.clone();
    let db_for_push = state.db.clone();
    let http = state.http_client.clone();

    // The soft-delete transaction and the sync-target gates (SQLite pool
    // checkout, config load, OS keychain read) are both blocking — one
    // blocking hop for both, mirroring `delete_all_recordings`.
    let (ids, parts) = tokio::task::spawn_blocking(move || -> AppResult<SoftDeleteAllOutcome> {
        let conn = db.conn()?;
        let ids = RecordingsRepo::soft_delete_created_between(&conn, &start_iso, &end_iso)
            .map_err(AppError::from)?;
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
        "Move by date range to Trash: recordings moved (30-day recovery window)"
    );
    Ok(DeleteAllResult {
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
    tokio::task::spawn_blocking(move || import_audio_file_inner(&db, &data_dir, &file_path))
        .await
        .map_err(join_err)?
}

/// Best-effort cleanup of a partial import destination: the copy/convert
/// can leave a partially written plaintext WAV before failing — no row
/// references it and no sweep covers it (the orphan-WAV sweep only
/// ENCRYPTS rowless files, days later). Shred it; a NotFound (nothing was
/// ever written) is fine. Logs the failure only — never the path (PHI).
fn shred_partial_import(dest: &std::path::Path) {
    if let Err(e) = medical_security::file_crypto::shred_and_unlink(dest)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "import: cleaning up partial destination failed");
    }
}

/// Blocking core of [`import_audio_file`], extracted so the failed-import
/// cleanup is unit-testable without `tauri::State`.
fn import_audio_file_inner(
    db: &Arc<medical_db::Database>,
    data_dir: &std::path::Path,
    file_path: &str,
) -> AppResult<String> {
    let source = PathBuf::from(file_path);
    if !source.exists() {
        return Err(AppError::Other(format!("File not found: {file_path}")));
    }

    // Resolve recordings directory from settings (custom path or default).
    let recordings_dir = resolve_recordings_dir(db, data_dir)?;

    let original_name = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".to_string());

    let recording_id = Uuid::new_v4();
    let short_id = &recording_id.to_string()[..8];

    // Destination for both paths (copy or convert). On ANY error from here
    // until the row insert commits, the partial destination is shredded —
    // a half-written plaintext WAV is PHI on disk that nothing references.
    let dest_path = recordings_dir.join(format!("{original_name}_{short_id}.wav"));
    let is_wav_source = medical_audio::convert::is_wav_file(&source);
    let copy_or_convert: AppResult<()> = if is_wav_source {
        // Already WAV — just copy.
        std::fs::copy(&source, &dest_path)
            .map(|_| ())
            .map_err(|e| AppError::audio(format!("Failed to copy file: {e}")))
    } else {
        // Non-WAV — convert to WAV.
        medical_audio::convert::convert_to_wav(&source, &dest_path)
            .map_err(|e| AppError::audio(format!("Failed to convert audio: {e}")))
    };
    if let Err(e) = copy_or_convert {
        shred_partial_import(&dest_path);
        return Err(e);
    }

    // Read duration and file size from the resulting WAV. If the just-written
    // WAV is unreadable, that's a real signal (corrupt source, converter bug)
    // — surface it instead of silently setting duration=None.
    let file_size = match std::fs::metadata(&dest_path) {
        Ok(m) => m.len(),
        Err(e) => {
            shred_partial_import(&dest_path);
            return Err(AppError::audio(format!("imported WAV unreadable: {e}")));
        }
    };
    let duration = {
        match hound::WavReader::open(&dest_path) {
            Ok(reader) => {
                let spec = reader.spec();
                let total_samples = reader.len() as f64;
                if spec.sample_rate > 0 && spec.channels > 0 {
                    total_samples / (spec.sample_rate as f64 * spec.channels as f64)
                } else {
                    0.0
                }
            }
            Err(e) => {
                shred_partial_import(&dest_path);
                return Err(AppError::audio(format!("imported WAV unreadable: {e}")));
            }
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

    let insert_result: AppResult<()> = (|| {
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
        Ok(())
    })();
    if let Err(e) = insert_result {
        // No row survived — nothing references the destination.
        shred_partial_import(&recording.audio_path);
        return Err(e);
    }

    if file_size > 0 {
        match medical_security::file_crypto::encrypt_file_in_place(&recording.audio_path) {
            Ok(()) => {
                let conn = db.conn()?;
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
                        // Id + error only — the import path embeds the
                        // user's filename, which is PHI per the logging
                        // rules (counts/lengths/ids, never paths).
                        tracing::warn!(error = %e, recording_id = %recording_id, "import: could not encrypt; encryption_pending stays set for the boot sweep")
                    }
                }
            }
        }
    }

    Ok(recording_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed conversion must leave NO destination file: the old path
    /// returned the error with a partial plaintext WAV stranded in the
    /// recordings dir (no row references it; the orphan sweep would only
    /// encrypt it days later).
    #[test]
    fn failed_conversion_leaves_no_destination_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Short non-WAV garbage: passes the exists() check, fails
        // `convert_to_wav` deterministically at format probing.
        let src = tmp.path().join("corrupt-audio.mp3");
        std::fs::write(&src, b"definitely not audio data").expect("write corrupt source");

        let db = Arc::new(medical_db::Database::open_in_memory().expect("db"));
        let result = import_audio_file_inner(&db, tmp.path(), src.to_str().unwrap());
        assert!(result.is_err(), "garbage non-WAV source must fail");

        let recordings_dir = tmp.path().join("recordings");
        let leftovers: Vec<_> = std::fs::read_dir(&recordings_dir)
            .expect("recordings dir exists (resolver created it)")
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "partial destination must be shredded on a failed conversion, found {leftovers:?}"
        );
    }

    /// The composite query's interval bounds: both halves together or
    /// neither, always parseable, never reversed — and normalized to UTC
    /// so SQL compares like-for-like.
    #[test]
    fn query_interval_bounds_accept_valid_pairs_normalized() {
        let bounds = query_interval_bounds(
            Some("2026-10-01T02:00:00+02:00"),
            Some("2026-10-02T00:00:00Z"),
        )
        .expect("valid pair");
        assert_eq!(
            bounds,
            Some((
                "2026-10-01T00:00:00+00:00".to_string(),
                "2026-10-02T00:00:00+00:00".to_string(),
            )),
            "offsets normalize to UTC RFC3339"
        );

        assert_eq!(
            query_interval_bounds(None, None).expect("absent pair is no filter"),
            None
        );
    }

    #[test]
    fn query_interval_bounds_reject_incomplete_pairs() {
        let err = query_interval_bounds(Some("2026-10-01T00:00:00Z"), None)
            .expect_err("start without end must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
        let err = query_interval_bounds(None, Some("2026-10-02T00:00:00Z"))
            .expect_err("end without start must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }

    #[test]
    fn query_interval_bounds_reject_malformed_and_reversed() {
        let err = query_interval_bounds(Some("yesterday"), Some("2026-10-02T00:00:00Z"))
            .expect_err("malformed start must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");

        let err = query_interval_bounds(Some("2026-10-01T00:00:00Z"), Some("not-a-timestamp"))
            .expect_err("malformed end must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");

        let err = query_interval_bounds(Some("2026-10-02T00:00:00Z"), Some("2026-10-01T00:00:00Z"))
            .expect_err("reversed window must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");

        // Equal bounds are an empty window, not a valid one.
        let err = query_interval_bounds(Some("2026-10-01T00:00:00Z"), Some("2026-10-01T00:00:00Z"))
            .expect_err("zero-width window must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }

    /// Limit clamping for the composite query: default 50, capped at 200 —
    /// an absurd caller request must not fetch the whole table.
    #[test]
    fn query_limit_defaults_and_caps() {
        assert_eq!(query_limit_or_default(None), 50);
        assert_eq!(query_limit_or_default(Some(10)), 10);
        assert_eq!(query_limit_or_default(Some(200)), 200);
        assert_eq!(query_limit_or_default(Some(10_000)), 200);
    }

    /// Purge-Now candidate parsing: a malformed id or an identity-less
    /// candidate must be rejected BEFORE any DB work — never silently
    /// dropped (the purge would then under-deliver vs its confirmation)
    /// and never defaulted (a fabricated identity could purge an
    /// unreviewed deletion).
    #[test]
    fn parse_trash_purge_candidates_round_trips_valid_set() {
        let dtos = vec![
            TrashPurgeCandidateDto {
                id: "3fa85f64-5717-4562-b3fc-2c963f66afa6".into(),
                deleted_at: "2026-11-01T00:00:00+00:00".into(),
            },
            TrashPurgeCandidateDto {
                id: "0c0f1a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b".into(),
                deleted_at: "2026-11-02T12:34:56.789+02:00".into(),
            },
        ];
        let parsed = parse_trash_purge_candidates(dtos).expect("valid set parses");
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].id.to_string(),
            "3fa85f64-5717-4562-b3fc-2c963f66afa6"
        );
        // Identity travels byte-for-byte — any normalization would break
        // the backend's raw-string recheck.
        assert_eq!(parsed[1].deleted_at, "2026-11-02T12:34:56.789+02:00");
    }

    #[test]
    fn parse_trash_purge_candidates_rejects_bad_ids_and_empty_identity() {
        let err = parse_trash_purge_candidates(vec![TrashPurgeCandidateDto {
            id: "not-a-uuid".into(),
            deleted_at: "2026-11-01T00:00:00+00:00".into(),
        }])
        .expect_err("malformed id must be rejected");
        assert!(matches!(err, AppError::Other(_)), "{err}");

        let err = parse_trash_purge_candidates(vec![TrashPurgeCandidateDto {
            id: "3fa85f64-5717-4562-b3fc-2c963f66afa6".into(),
            deleted_at: "   ".into(),
        }])
        .expect_err("blank identity must be rejected");
        assert!(matches!(err, AppError::InvalidInput(_)), "{err}");
    }
    /// PHI-shaped markers for the diagnostic leak tests below. Synthetic — no
    /// real patient data, but exactly the class of content that can ride inside
    /// an `AppError::Database` message (the `DbError` Display) or the wrapped
    /// rusqlite engine string.
    const PHI_MARKER_MESSAGE: &str = "PHI-MARKER-doe-jane-205-pandosy";
    const PHI_MARKER_SQLITE: &str = "PHI-MARKER-near-visit-notes.wav";

    /// Capture the tracing output of one `log_purge_failure_diagnostic` call
    /// into a string, with a scoped subscriber (no global default touched, so
    /// this is safe to run alongside the rest of the suite).
    fn capture_purge_diagnostic(err: &AppError) -> String {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct CaptureWriter(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for CaptureWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        struct CaptureMaker(Arc<Mutex<Vec<u8>>>);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureMaker {
            type Writer = CaptureWriter;
            fn make_writer(&'a self) -> Self::Writer {
                CaptureWriter(self.0.clone())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(CaptureMaker(buf.clone()))
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || log_purge_failure_diagnostic(err));
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Privacy regression (Frikkie pre-review correction): the Purge Now
    /// failure diagnostic must NEVER emit the raw error — a Database error
    /// whose message and rusqlite engine string both carry PHI-shaped markers
    /// must log only the static operation, the stable kind discriminant, and
    /// the numeric SQLite extended code. Pre-fix this failed: the raw
    /// `error = %e` Display carried both markers into the log line.
    #[test]
    fn purge_failure_diagnostic_emits_no_raw_database_error_content() {
        let sqlite_err = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseCorrupt,
                extended_code: 267, // SQLITE_CORRUPT_VTAB — the field-report class
            },
            Some(PHI_MARKER_SQLITE.to_string()),
        );
        // Exactly how the purge path wraps DB failures (crates/db lib.rs
        // `From<DbError> for AppError`): message = DbError Display, source =
        // the typed DbError with the rusqlite error beneath it.
        let db_err = medical_db::DbError::Sqlite(sqlite_err);
        let err = AppError::database_with_source(db_err.to_string(), db_err);

        let captured = capture_purge_diagnostic(&err);

        assert!(
            !captured.contains(PHI_MARKER_MESSAGE),
            "raw DbError message must not reach the log, got: {captured}"
        );
        assert!(
            !captured.contains(PHI_MARKER_SQLITE),
            "raw rusqlite engine string must not reach the log, got: {captured}"
        );
        assert!(
            !captured.contains("SQLite error"),
            "DbError Display prefix carries content risk, got: {captured}"
        );
        // The safe class MUST be present: static operation, kind, numeric code.
        assert!(
            captured.contains("purge_trash_now"),
            "operation: {captured}"
        );
        assert!(
            captured.contains("Database"),
            "kind discriminant: {captured}"
        );
        assert!(captured.contains("267"), "numeric sqlite code: {captured}");
    }

    /// The fallback for UNCLASSIFIED errors (no sqlite code in the chain) is
    /// the static operation + kind only — never a raw Display of the error.
    #[test]
    fn purge_failure_diagnostic_fallback_is_static_not_raw() {
        let err = AppError::Other(format!("unclassified failure {PHI_MARKER_MESSAGE}"));
        let captured = capture_purge_diagnostic(&err);

        assert!(
            !captured.contains(PHI_MARKER_MESSAGE),
            "unclassified fallback must not emit raw Display, got: {captured}"
        );
        assert!(
            captured.contains("purge_trash_now"),
            "operation: {captured}"
        );
        assert!(captured.contains("Other"), "kind discriminant: {captured}");
    }

    /// The numeric-code extractor must find the code through the real wrap
    /// chain (AppError → DbError → rusqlite::Error::SqliteFailure) and return
    /// None for chains without one.
    #[test]
    fn sqlite_extended_code_walks_the_real_wrap_chain() {
        let db_err = medical_db::DbError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseCorrupt,
                extended_code: 267,
            },
            None,
        ));
        let err = AppError::database_with_source(db_err.to_string(), db_err);
        assert_eq!(sqlite_extended_code(&err), Some(267));

        let plain = AppError::Other("no chain".into());
        assert_eq!(sqlite_extended_code(&plain), None);
    }
}
