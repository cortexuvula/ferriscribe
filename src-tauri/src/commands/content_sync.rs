//! Tauri commands for content sync (recordings, transcripts, SOAP, ...).
//!
//! Dispatch model: content-sync operations check three gates before routing
//! through the office server's HTTP API:
//!
//! 1. The `sync_content` opt-in setting must be enabled.
//! 2. This client must be paired with an office server that exposes a
//!    `vocab` port AND advertises a Tailscale address. Content sync routes
//!    **exclusively over Tailscale** — never the LAN — because the payload
//!    is PHI.
//! 3. A bearer token must be present.
//!
//! When all three hold, [`sync_content_now`] performs a bidirectional merge
//! and [`subscribe_content_sync`] keeps this client near-realtime via SSE.
//! When any gate fails, the commands return quietly and the app operates
//! against the local SQLite store only.
//!
//! # HIPAA note
//!
//! No transcript / SOAP / referral / letter / chat / audio content is logged.
//! Logging is restricted to counts, IDs, and byte lengths.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tauri::Emitter;
use tracing::instrument;

use medical_core::error::{AppError, AppResult};
use medical_db::Database;
use medical_db::content_sync::{
    ContentSyncRepo, FieldRevision, SYNCABLE_FIELDS, SyncFieldValue, SyncRecording,
};
use medical_db::recordings::RecordingsRepo;

use crate::commands::sharing::PairedConnection;
use crate::state::{self, AppState};

/// Build the next composite keyset cursor from a batch boundary.
///
/// After a push/pull batch succeeds, the cursor must encode the
/// last-delivered position so `changed_since` can resume exactly after it.
/// A bare timestamp can't do that: bulk writers like `soft_delete_all`
/// stamp whole batches with one shared `updated_at`, and `julianday`
/// comparison (millisecond precision) can't see a +1µs nudge — so the
/// cursor is now `<updated_at>|<last-delivered-id>` and the selector pages
/// within a shared timestamp by id (see `ContentSyncRepo::changed_since`).
///
/// The batch's timestamp string is preserved verbatim: `changed_since`'s
/// tie arm compares `updated_at` strings for equality, and re-serializing
/// would normalize a `Z`-suffixed stamp to `+00:00` and silently break that
/// comparison. The old +1µs nudge is gone for the same reason — a nudged
/// timestamp no longer string-matches the rows it must tie-break against
/// (the id component subsumes what the nudge achieved).
///
/// Clock-skew clamp (2026-08-17 tracked item, fixed 2026-09-03): a server
/// row written by a machine with a fast clock carries a FUTURE timestamp;
/// advancing the cursor past it would pin every pull fleet-wide at that
/// future instant, and no machine's present-day writes would be `>` the
/// cursor until real time caught up — silently missed updates across the
/// whole practice. A future batch max is therefore clamped to the LOCAL
/// now with the id component left EMPTY (a plain timestamp — the legacy
/// cursor shape), so the bucket at that timestamp re-delivers on subsequent
/// pulls (LWW merges are idempotent, so the only cost is redundant
/// transfer) instead of skipping everyone else's rows.
///
/// If the input fails to parse it is returned unchanged with no id
/// component (the raw `max_ts` is still a safe-enough cursor — the
/// data-loss window only affects rows sharing that exact timestamp, which
/// re-deliver under the legacy shape).
fn advance_cursor(ts: &str, last_id: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return ts.to_string();
    };
    let local_now = chrono::Utc::now();
    if dt > local_now {
        tracing::warn!(
            "sync cursor clamped to local now — server row carries a future timestamp (clock skew?)"
        );
        return local_now.to_rfc3339();
    }
    format!("{ts}|{last_id}")
}

/// The batch boundary [`advance_cursor`] consumes: the maximum `updated_at`
/// in the batch plus the maximum id among the rows carrying that exact
/// timestamp. Every delivered row sorts at or before this `(timestamp, id)`
/// position in `changed_since`'s `(julianday, updated_at, id)` delivery
/// order, so the composite cursor resumes exactly after the batch.
fn batch_cursor_boundary(batch: &[SyncRecording]) -> Option<(String, String)> {
    batch
        .iter()
        .map(|r| (r.updated_at.as_str(), r.id.as_str()))
        .max_by(|a, b| a.cmp(b))
        .map(|(ts, id)| (ts.to_string(), id.to_string()))
}

/// Returns `Some((conn, bearer, http_client))` when content sync should route
/// through the office server. Three gates must all pass:
///
/// 1. `config.sync_content` is true (user opt-in).
/// 2. The paired connection has a Tailscale address **and** a vocab port.
///    (Tailscale-only transport — content sync never falls back to LAN.)
/// 3. A bearer token is present.
///
/// `pub(crate)` so other command files (e.g. a future recording-edit command
/// that wants to push on save) can reuse the same gating.
pub(crate) async fn content_sync_target(
    state: &AppState,
) -> Option<(PairedConnection, String, Arc<reqwest::Client>)> {
    let db = Arc::clone(&state.db);
    let http_client = state.http_client.clone();
    tokio::task::spawn_blocking(move || content_sync_target_parts(&db, http_client))
        .await
        .ok()
        .flatten()
}

/// Same gates as [`content_sync_target`], split out for `spawn_blocking`
/// call sites, which can't borrow `tauri::State` across the thread boundary.
/// Blocking-safe: config load (SQLite) + keychain read happen here.
pub(crate) fn content_sync_target_parts(
    db: &Arc<medical_db::Database>,
    http_client: Arc<reqwest::Client>,
) -> Option<(PairedConnection, String, Arc<reqwest::Client>)> {
    // Each gate logs WHY it failed at debug level, so a silently-zero sync is
    // diagnosable from the logs instead of being indistinguishable from
    // "synced, nothing changed". Debug (not info) because the per-edit push
    // paths call this too and would spam when gates are down.
    // Gate 1: user opt-in.
    let Ok(config) = crate::commands::settings::load_config_sync(db) else {
        tracing::debug!("content sync skipped: could not load settings");
        return None;
    };
    if !config.sync_content {
        tracing::debug!("content sync skipped: sync_content is disabled in settings");
        return None;
    }
    // Gate 2: paired connection with Tailscale + vocab port.
    let Some(conn) = state::load_paired_connection() else {
        tracing::debug!("content sync skipped: not paired with an office server");
        return None;
    };
    if conn.ports.vocab.is_none() {
        tracing::debug!(
            "content sync skipped: paired connection has no vocab port (server predates content sync?)"
        );
        return None;
    }
    if conn.tailscale.is_none() {
        tracing::debug!(
            "content sync skipped: paired connection has no Tailscale address \
             (re-pair, or ensure the server advertises Tailscale)"
        );
        return None;
    }
    // Gate 3: bearer token.
    let Some(bearer) = state::load_sharing_bearer() else {
        tracing::debug!("content sync skipped: no sharing bearer token (unpaired?)");
        return None;
    };
    Some((conn, bearer, http_client))
}

/// Build a sparse [`SyncRecording`] from a local recording row + its field
/// revisions, suitable for pushing to the server.
///
/// Mirrors the server-side `recording_to_sync` + `build_sparse_fields` logic.
/// Only fields with content are included (sparse by design) so absent fields
/// don't participate in the merge.
///
/// `pub(crate)` so the recording-edit commands can push a single updated
/// recording without going through a full sync round-trip.
pub(crate) fn build_sync_recording(
    conn: &rusqlite::Connection,
    rec_id: &str,
) -> AppResult<SyncRecording> {
    let uuid = uuid::Uuid::parse_str(rec_id)
        .map_err(|e| AppError::Other(format!("build_sync_recording: invalid recording id: {e}")))?;
    let rec = RecordingsRepo::get_by_id(conn, &uuid).map_err(AppError::from)?;

    // Read deleted_at separately — it's not on the Recording struct.
    let deleted_at: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            rusqlite::params![rec_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let revisions = ContentSyncRepo::revisions_for(conn, &uuid).map_err(AppError::from)?;

    // Strip the synced_from marker from metadata before building the wire
    // payload. This is a local-only flag — it must NOT be transmitted back
    // to the origin machine, or the origin would see its own recording as
    // "remote" after the next sync round-trip. Strip in memory only — do
    // NOT write back to DB (that would mutate local state as a side effect
    // of a push read, and would skip the revision-tracking system).
    let mut rec_clean = rec.clone();
    if let Some(obj) = rec_clean.metadata.as_object_mut() {
        obj.remove("synced_from");
    }

    let fields = build_sparse_fields(&rec_clean, &revisions);

    Ok(SyncRecording {
        id: rec.id.to_string(),
        filename: rec.filename.clone(),
        created_at: rec.created_at.to_rfc3339(),
        updated_at: rec
            .updated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| rec.created_at.to_rfc3339()),
        deleted_at,
        patient_name: rec.patient_name.clone(),
        duration_seconds: rec.duration_seconds,
        file_size_bytes: rec.file_size_bytes,
        stt_provider: rec.stt_provider.clone(),
        ai_provider: rec.ai_provider.clone(),
        fields,
    })
}

/// Build the sparse field map for a recording.
///
/// Delegates to the shared [`crate::sync_sparse_fields::build_sparse_fields`]
/// (single source of truth for both sync directions — the client push path
/// here and the server's pull responses).
fn build_sparse_fields(
    rec: &medical_core::types::recording::Recording,
    revisions: &[FieldRevision],
) -> HashMap<String, SyncFieldValue> {
    crate::sync_sparse_fields::build_sparse_fields(rec, revisions)
}

/// Audio uploads attempted per push batch — bounds one cycle's upload time.
const AUDIO_UPLOADS_PER_BATCH: usize = 10;

/// Append newly pushed ids to the persistent audio queue, skipping ids
/// already queued (a re-pushed recording must not duplicate its retry).
fn merge_audio_queue(queue: &[String], pushed: &[String]) -> Vec<String> {
    let mut out = queue.to_vec();
    for id in pushed {
        if !out.iter().any(|q| q == id) {
            out.push(id.clone());
        }
    }
    out
}

/// Persist the audio upload queue (ids only). Best-effort: a failed persist
/// means the queue re-drains from its last saved state — safe, because
/// uploads are idempotent (the server's first-write-wins 409 is treated as
/// success by `upload_audio`).
async fn persist_audio_queue(db: &Arc<Database>, ids: &[String]) {
    let db = Arc::clone(db);
    let ids = ids.to_vec();
    let result = tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.conn()?;
        ContentSyncRepo::set_pending_audio_uploads(&conn, &ids).map_err(AppError::from)
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "sync: persist audio queue failed"),
        Err(e) => tracing::warn!(error = %e, "sync: persist audio queue task failed"),
    }
}

/// Outcome of reading one queued recording's local audio for upload.
#[derive(Debug)]
enum PendingAudioRead {
    /// Decrypted (or legacy plaintext) audio bytes, ready to upload.
    Bytes(Vec<u8>),
    /// The recording is gone or tombstoned, or has no local audio file
    /// (e.g. a pulled recording whose audio still lives on the partner) —
    /// the id can never upload and must leave the queue.
    Gone,
    /// The audio exists but could not be read/decrypted — possibly
    /// transient (keychain); keep the id for a later cycle.
    Failed(AppError),
}

/// Read one recording's local audio for upload. Runs on the blocking pool.
fn read_local_audio(db: &Arc<Database>, rec_id: &str) -> PendingAudioRead {
    let conn = match db.conn() {
        Ok(c) => c,
        Err(e) => return PendingAudioRead::Failed(e.into()),
    };
    // A malformed id can never resolve — drop it rather than retry forever.
    let Ok(uuid) = uuid::Uuid::parse_str(rec_id) else {
        return PendingAudioRead::Gone;
    };
    // Active rows only: a tombstoned recording must not upload its audio
    // (the tombstone push already told the partner it is deleted).
    let rec = match RecordingsRepo::get_by_id_active(&conn, &uuid) {
        Ok(rec) => rec,
        Err(medical_db::DbError::NotFound(_)) => return PendingAudioRead::Gone,
        Err(e) => return PendingAudioRead::Failed(e.into()),
    };
    let path = &rec.audio_path;
    if path.as_os_str().is_empty() || !path.exists() {
        return PendingAudioRead::Gone;
    }
    match medical_security::file_crypto::decrypt_file(path) {
        Ok(bytes) => PendingAudioRead::Bytes(bytes),
        Err(medical_security::file_crypto::FileCryptoError::NotEncrypted) => {
            // Legacy plaintext WAV — auto-detected by the missing magic.
            match std::fs::read(path) {
                Ok(bytes) => PendingAudioRead::Bytes(bytes),
                Err(e) => {
                    PendingAudioRead::Failed(AppError::Other(format!("audio read failed: {e}")))
                }
            }
        }
        Err(e) => {
            PendingAudioRead::Failed(AppError::security(format!("audio decrypt failed: {e}")))
        }
    }
}

/// Upper bound on the persisted audio-fetch skip set. The OLDEST entries
/// (longest-tenured skips) are dropped first, so a permanently-skipped id
/// eventually re-enters selection and is retried — the set bounds state,
/// not forever-ness.
const AUDIO_FETCH_SKIP_CAP: usize = 100;

/// The audio-fetch skip set's in-memory shape: `(id, consecutive-miss
/// count)` pairs, ordered oldest-entry-first (tenure — see
/// [`AUDIO_FETCH_SKIP_CAP`]).
type AudioFetchSkips = Vec<(String, u64)>;

/// The typed "no audio on the server" outcome the audio GET endpoint
/// returns via `ContentRemote::fetch_audio` (whose error surface is
/// stringly `AppError::Other`, so the discriminator is the producer's
/// message). This is the ONLY failure shape that may enter the skip set: a
/// transport/auth/5xx failure is potentially transient and must retry next
/// cycle, while "row exists, file never arrives" is the permanent shape
/// that head-of-line blocks later rows.
fn is_no_audio_on_server(err: &AppError) -> bool {
    err.to_string().contains("no audio on the office server")
}

/// Record one consecutive "no audio on the server" miss for `rec_id`:
/// increment the existing entry in place (list position = tenure is kept)
/// or append the id, capping the set by dropping the OLDEST entries.
fn bump_audio_fetch_skip(skips: &mut AudioFetchSkips, rec_id: &str) {
    if let Some((_, count)) = skips.iter_mut().find(|(id, _)| id == rec_id) {
        *count += 1;
        return;
    }
    skips.push((rec_id.to_string(), 1));
    while skips.len() > AUDIO_FETCH_SKIP_CAP {
        skips.remove(0);
    }
}

/// Drop `rec_id` from the skip set (fetch succeeded, or the row is gone).
/// Returns whether the set changed and needs persisting.
fn clear_audio_fetch_skip(skips: &mut AudioFetchSkips, rec_id: &str) -> bool {
    let before = skips.len();
    skips.retain(|(id, _)| id != rec_id);
    skips.len() != before
}

/// Live rows still missing local audio, oldest first, EXCLUDING the skip
/// set. The exclusion lives in SQL so the LIMIT applies to ATTEMPTABLE
/// rows: filtering after the LIMIT would let permanently-skipped ids keep
/// holding selection slots — the exact head-of-line block this exists to
/// break.
fn select_empty_audio_ids(
    conn: &rusqlite::Connection,
    skip_ids: &[String],
) -> AppResult<Vec<String>> {
    let (sql, params): (String, Vec<&dyn rusqlite::ToSql>) = if skip_ids.is_empty() {
        (
            "SELECT id FROM recordings
              WHERE audio_path = '' AND deleted_at IS NULL
              ORDER BY created_at ASC LIMIT 10"
                .to_string(),
            Vec::new(),
        )
    } else {
        let placeholders = skip_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        (
            format!(
                "SELECT id FROM recordings
                  WHERE audio_path = '' AND deleted_at IS NULL
                    AND id NOT IN ({placeholders})
                  ORDER BY created_at ASC LIMIT 10"
            ),
            skip_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect(),
        )
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
    let ids = stmt
        .query_map(params.as_slice(), |row| row.get::<_, String>(0))
        .map_err(|e| AppError::from(medical_db::DbError::from(e)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

/// Prune skip entries whose rows are no longer empty-audio live rows
/// (audio arrived via the manual fetch command, the row was tombstoned,
/// purged, or deleted): a skip must never outlive its usefulness, or a
/// later legitimate fetch of that row would stay suppressed forever.
fn prune_audio_fetch_skips(
    conn: &rusqlite::Connection,
    skips: AudioFetchSkips,
) -> AppResult<AudioFetchSkips> {
    if skips.is_empty() {
        return Ok(skips);
    }
    let placeholders = skips.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id FROM recordings
          WHERE audio_path = '' AND deleted_at IS NULL AND id IN ({placeholders})"
    );
    let ids: Vec<&str> = skips.iter().map(|(id, _)| id.as_str()).collect();
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
    let still_candidates: std::collections::HashSet<String> = stmt
        .query_map(params.as_slice(), |row| row.get::<_, String>(0))
        .map_err(|e| AppError::from(medical_db::DbError::from(e)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(skips
        .into_iter()
        .filter(|(id, _)| still_candidates.contains(id))
        .collect())
}

/// Run one full bidirectional content sync against the office server.
///
/// This is the core logic shared by the [`sync_content_now`] command and the
/// [`run_initial_sync`] startup hook. It:
///
/// 1. **Pull loop**: read the local cursor, call `remote.pull(cursor)`, merge
///    the incoming batch into the local store (per-field LWW), advance the
///    cursor to the batch's max `updated_at`, and repeat while `has_more`.
/// 2. **Push**: collect local recording IDs changed since the cursor, build
///    `SyncRecording`s for each, and push them in a single batch.
///
/// Returns a summary (counts only — never PHI). Errors are logged and
/// propagated so the caller can decide whether to surface them.
async fn run_sync(
    db: Arc<Database>,
    data_dir: &std::path::Path,
    remote: &crate::content_remote::ContentRemote<'_>,
    app: &tauri::AppHandle,
) -> AppResult<SyncSummary> {
    let mut summary = SyncSummary::default();

    // ── Backfill NULL updated_at ───────────────────────────────────────
    // The migration that adds updated_at (m013) backfills existing rows,
    // but edge cases (interrupted migration, direct DB edits) can leave
    // NULLs. NULL updated_at rows are excluded by `changed_since`'s strict
    // `>` comparison, so they'd be invisible to incremental sync. Backfill
    // them here so they're visible to both pull and push.
    {
        let backfill_db = Arc::clone(&db);
        let _ = tokio::task::spawn_blocking(move || -> AppResult<()> {
            let conn = backfill_db.conn()?;
            conn.execute(
                "UPDATE recordings SET updated_at = created_at WHERE updated_at IS NULL",
                [],
            )
            .map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
            Ok(())
        })
        .await;
    }

    // ── Pull loop ───────────────────────────────────────────────────────
    loop {
        // Read cursor and pull a batch on the blocking pool, then merge.
        let cursor = tokio::task::spawn_blocking({
            let db = Arc::clone(&db);
            move || -> AppResult<Option<String>> {
                let conn = db.conn()?;
                Ok(ContentSyncRepo::get_cursor(&conn)
                    .map_err(AppError::from)?
                    .cursor)
            }
        })
        .await
        .map_err(crate::commands::join_err)??;

        let batch = remote.pull(cursor.as_deref()).await?;
        let batch_count = batch.recordings.len();
        let has_more = batch.has_more;

        // Merge incoming + advance the cursor. The next cursor is the batch
        // boundary — max `updated_at` plus the max id among the rows
        // carrying it (the server returns rows ordered by updated_at
        // ascending; the composite encodes the last-delivered position so
        // same-timestamp overflow rows aren't stranded past the batch limit).
        let next_cursor = batch_cursor_boundary(&batch.recordings);

        // Purge notifications travel on the same response; they are applied
        // on the same connection right after a successful merge (below).
        // Destructured out of `batch` so the merge closure can own both.
        let batch_recordings = batch.recordings;
        let batch_purged = batch.purged;
        let batch_purged_count = batch_purged.len();

        let merge_db = Arc::clone(&db);
        let merged = tokio::task::spawn_blocking(
            move || -> AppResult<(medical_db::content_sync::MergeResult, AppResult<()>)> {
                let conn = merge_db.conn()?;
                let result = ContentSyncRepo::merge_incoming(&conn, &batch_recordings)
                    .map_err(AppError::from)?;
                // Only reached after a successful merge. Tombstone any stale
                // LOCAL LIVE copy of a server-purged recording so this
                // machine converges with the practice-wide deletion. The
                // outcome is returned separately: the caller must hold the
                // cursor when it fails (see below) rather than treat it as
                // a merge failure.
                let purged_apply = ContentSyncRepo::apply_purged_refs(&conn, &batch_purged)
                    .map_err(AppError::from);
                Ok((result, purged_apply))
            },
        )
        .await
        .map_err(crate::commands::join_err)?;

        // If the merge failed, do NOT advance the cursor — break out of the
        // pull loop so the next sync cycle retries the same batch from the
        // same cursor position. Advancing past a failed merge would
        // permanently skip the failed batch (data loss).
        let (merge_result, purged_apply) = match merged {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    batch_count,
                    "sync: pull merge failed — NOT advancing cursor, will retry next cycle"
                );
                // Exit pull loop; cursor stays at pre-batch position so the
                // failed batch is retried on the next sync cycle.
                break;
            }
        };

        summary.pulled += batch_count;
        summary.merge_conflicts += merge_result.conflicts.len();

        // Emit per-recording update events so the editor can refresh (C6 fix).
        for id in &merge_result.changed_recording_ids {
            let _ = app.emit("recording-updated", serde_json::json!({ "id": id }));
        }

        // Purge application is best-effort for the SYNC (a failure never
        // fails the round), but it must hold the cursor: the next cursor is
        // the batch's max `updated_at`, which can exceed the failed refs'
        // `purged_at` — advancing would make the server consider them
        // already seen and they would never be re-delivered. Break without
        // advancing so the next cycle unconditionally retries both the
        // batch (idempotent re-merge) and the refs (idempotent no-ops on
        // already-tombstoned rows).
        if let Err(e) = purged_apply {
            tracing::warn!(
                purged_count = batch_purged_count,
                batch_count,
                error = %e,
                "sync: failed to apply purge notifications — NOT advancing cursor, will retry next cycle"
            );
            break;
        }

        // Advance the cursor if we made progress.
        if let Some((nc_ts, nc_id)) = next_cursor {
            let cursor_db = Arc::clone(&db);
            let nc = advance_cursor(&nc_ts, &nc_id);
            tokio::task::spawn_blocking(move || {
                let conn = cursor_db.conn()?;
                ContentSyncRepo::set_cursor(&conn, Some(&nc)).map_err(AppError::from)
            })
            .await
            .map_err(crate::commands::join_err)??;
        }

        if !has_more || batch_count == 0 {
            break;
        }
    }

    // ── Audio fetch for newly-synced recordings ────────────────────────
    // After pulling metadata, fetch audio for recordings that arrived
    // without it (audio_path is empty). Best-effort per recording: fetch
    // and write errors are logged and don't abort the sync (limit 10 per
    // cycle to bound latency); a DB-level selection failure still fails
    // the round so the cursor doesn't advance past unmerged state.
    sync_fetch_missing_audio(&db, data_dir, &remote.client).await?;

    // ── Push ────────────────────────────────────────────────────────────
    // Use a SEPARATE push cursor (independent from the pull cursor) so that
    // local recordings created before the first pull are still pushed.
    // The pull cursor tracks what we've received from the server; the push
    // cursor tracks what we've sent to the server. Without this separation,
    // the pull loop would advance the shared cursor past local recordings,
    // and they'd never be pushed.
    //
    // Audio uploads ride a PERSISTENT retry queue: every successfully
    // pushed recording is enqueued (surviving restarts and multi-batch
    // catch-ups), a bounded slice is attempted per batch, and only success
    // or a permanently-unuploadable id leaves the queue. Before the queue,
    // a take(10)-per-batch slice meant recordings past the tenth in a batch
    // had their audio silently never uploaded once the cursor advanced —
    // and failed uploads were never retried.
    // A failed queue READ aborts the push phase (propagated): proceeding
    // with an empty queue and a later successful persist would overwrite
    // the saved retries — losing exactly the backlog this queue exists to
    // protect. The next sync cycle retries.
    let mut audio_queue: Vec<String> = {
        let q_db = Arc::clone(&db);
        tokio::task::spawn_blocking(move || -> AppResult<Vec<String>> {
            let conn = q_db.conn()?;
            ContentSyncRepo::get_pending_audio_uploads(&conn).map_err(AppError::from)
        })
        .await
        .map_err(crate::commands::join_err)??
    };
    // Whether the push loop's final iteration had no batch — the steady
    // state, where the queued upload retries still need their one bounded
    // drain for the round (see the drain call after the loop). Assigned at
    // the top of every iteration, so it is always initialized when read.
    let mut last_batch_was_empty;
    loop {
        let push_db = Arc::clone(&db);
        let push_result = tokio::task::spawn_blocking(move || {
            let conn = push_db.conn()?;
            let push_cursor = ContentSyncRepo::get_push_cursor(&conn).map_err(AppError::from)?;
            let (ids, has_more) =
                ContentSyncRepo::changed_since(&conn, push_cursor.as_deref(), 200)
                    .map_err(AppError::from)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match build_sync_recording(&conn, id) {
                    Ok(sr) => out.push(sr),
                    Err(e) => {
                        tracing::warn!(
                            recording_id_len = id.len(),
                            error = %e,
                            "content sync push: skipping unreadable recording"
                        );
                    }
                }
            }
            // If the batch is empty but there were IDs, we need the batch
            // boundary of those IDs (max updated_at + max id at that
            // timestamp) so we can advance the push cursor past them.
            // Otherwise the push loop will livelock, retrying the same
            // unreadable recordings on every sync forever.
            let skip_cursor = if out.is_empty() && !ids.is_empty() {
                // Query the batch boundary of the IDs that failed to build.
                let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT updated_at, id FROM recordings WHERE id IN ({placeholders})
                     ORDER BY updated_at DESC, id DESC LIMIT 1"
                );
                let params: Vec<&dyn rusqlite::ToSql> =
                    ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                conn.query_row(&sql, params.as_slice(), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .ok()
            } else {
                None
            };
            Ok::<_, AppError>((out, has_more, skip_cursor))
        })
        .await
        .map_err(crate::commands::join_err)??;

        let has_more = push_result.1;
        let batch = push_result.0;
        let skip_cursor = push_result.2;
        let batch_was_empty = batch.is_empty();
        last_batch_was_empty = batch_was_empty;

        if !batch_was_empty {
            let push_count = batch.len();
            // Capture recording IDs and the batch boundary BEFORE moving batch.
            let pushed_ids: Vec<String> = batch.iter().map(|r| r.id.clone()).collect();
            let boundary = batch_cursor_boundary(&batch);
            let push_resp = match remote.push(batch).await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        batch_count = push_count,
                        "sync: push batch failed — NOT advancing cursor, will retry next cycle"
                    );
                    // Exit push loop; cursor stays at pre-batch position so the
                    // failed batch is retried on the next sync cycle instead of
                    // being permanently skipped.
                    break;
                }
            };
            summary.pushed += push_count;
            summary.push_conflicts += push_resp.conflicts.len();
            // Advance the push cursor so we don't re-push these next time.
            if let Some((ts, id)) = boundary {
                let pc_db = Arc::clone(&db);
                let ts = advance_cursor(&ts, &id);
                tokio::task::spawn_blocking(move || -> AppResult<()> {
                    let conn = pc_db.conn()?;
                    ContentSyncRepo::set_push_cursor(&conn, &ts).map_err(AppError::from)
                })
                .await
                .map_err(crate::commands::join_err)??;
            }
            // Enqueue EVERY pushed recording's audio — the cursor is about
            // to advance past the whole batch, so this is the only moment
            // these ids enter the queue. Persist immediately: a crash
            // between here and the uploads must not lose them (the push
            // cursor has already moved).
            audio_queue = merge_audio_queue(&audio_queue, &pushed_ids);
            persist_audio_queue(&db, &audio_queue).await;

            // Drain a bounded slice (oldest first) so one sync cycle can't
            // spend minutes uploading a catch-up backlog.
            drain_audio_uploads(&db, &mut audio_queue, |id, bytes| async move {
                remote.upload_audio(&id, bytes).await
            })
            .await;
        } else if let Some((ts, id)) = skip_cursor {
            // All recordings in this page were unreadable — advance the push
            // cursor past them so they're not retried on every sync.
            let cursor = advance_cursor(&ts, &id);
            tracing::warn!(cursor = %cursor, "content sync push: advancing cursor past unreadable recordings");
            let pc_db = Arc::clone(&db);
            tokio::task::spawn_blocking(move || -> AppResult<()> {
                let conn = pc_db.conn()?;
                ContentSyncRepo::set_push_cursor(&conn, &cursor).map_err(AppError::from)
            })
            .await
            .map_err(crate::commands::join_err)??;
        }

        if !has_more || batch_was_empty {
            break;
        }
    }
    // The steady state: the loop exited without a batch to push (no local
    // changes). The drain above only ran inside a pushed batch, so queued
    // upload retries would NEVER fire while local changes are quiet — run
    // the same bounded drain once for the round instead.
    if last_batch_was_empty {
        drain_audio_uploads(&db, &mut audio_queue, |id, bytes| async move {
            remote.upload_audio(&id, bytes).await
        })
        .await;
    }

    Ok(summary)
}

/// Attempt up to [`AUDIO_UPLOADS_PER_BATCH`] queued audio uploads, oldest
/// first. Success or a permanently-unuploadable id (`PendingAudioRead::Gone`)
/// leaves the queue; transient failures (upload error, unreadable audio,
/// task join failure) keep their id for a later cycle — one bounded attempt
/// per id per drain. Removals are persisted before returning.
///
/// Extracted so BOTH push-loop outcomes drain: after a successfully pushed
/// batch, and once when the round exits without any batch to push (the
/// steady state — no local changes — where the per-batch drain never runs
/// but the queue's retries still deserve their attempt).
async fn drain_audio_uploads<F, Fut>(
    db: &Arc<Database>,
    audio_queue: &mut Vec<String>,
    mut upload: F,
) where
    F: FnMut(String, Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = AppResult<()>>,
{
    let attempt: Vec<String> = audio_queue
        .iter()
        .take(AUDIO_UPLOADS_PER_BATCH)
        .cloned()
        .collect();
    let mut remove_ids: Vec<String> = Vec::new();
    for rec_id in &attempt {
        let upload_db = Arc::clone(db);
        let rec_id_owned = rec_id.clone();
        let plaintext_result =
            tokio::task::spawn_blocking(move || read_local_audio(&upload_db, &rec_id_owned))
                .await
                .map_err(crate::commands::join_err);
        match plaintext_result {
            // Join failure is transient (task panicked / runtime shutdown)
            // — keep the id.
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    recording_id = %rec_id,
                    "sync: audio read task failed — queued for retry"
                );
            }
            Ok(read) => match read {
                PendingAudioRead::Bytes(plaintext) => {
                    match upload(rec_id.clone(), plaintext).await {
                        Ok(()) => {
                            remove_ids.push(rec_id.clone());
                        }
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                recording_id = %rec_id,
                                "sync: audio upload failed — queued for retry"
                            );
                        }
                    }
                }
                PendingAudioRead::Gone => {
                    // Row deleted, tombstoned, or no local audio (e.g. a
                    // pulled recording whose audio still lives on the
                    // partner) — this id can never upload.
                    tracing::debug!(
                        recording_id = %rec_id,
                        "sync: audio upload dropped — no uploadable local audio"
                    );
                    remove_ids.push(rec_id.clone());
                }
                PendingAudioRead::Failed(e) => {
                    // At-rest corruption or a keychain failure — retried
                    // next cycle (bounded: one attempt per cycle) and
                    // diagnosable via the warn.
                    tracing::warn!(
                        error = %e,
                        recording_id = %rec_id,
                        "sync: audio upload deferred — local audio unreadable"
                    );
                }
            },
        }
    }
    if !remove_ids.is_empty() {
        audio_queue.retain(|id| !remove_ids.contains(id));
        persist_audio_queue(db, audio_queue).await;
    }
}

/// Post-pull audio catch-up inside [`run_sync`]: fetch audio for recordings
/// whose metadata arrived via sync without the blob (`audio_path` is
/// empty). Best-effort — per-recording failures are logged and never abort
/// the round; a DB-level selection failure propagates to the caller (the
/// round fails and retries, matching the pre-extraction behavior).
///
/// Rows the server answers "no audio on the server" enter the persisted
/// skip set (see [`AUDIO_FETCH_SKIP_CAP`]) so a permanently-missing blob
/// cannot head-of-line block the later rows behind it; every other failure
/// shape retries next cycle.
async fn sync_fetch_missing_audio(
    db: &Arc<Database>,
    data_dir: &std::path::Path,
    client: &std::sync::Arc<reqwest::Client>,
) -> AppResult<()> {
    let audio_conn = crate::state::load_paired_connection_offload().await;
    let audio_tailscale = audio_conn.as_ref().and_then(|c| c.tailscale.clone());
    let audio_vocab_port = audio_conn.as_ref().and_then(|c| c.ports.vocab);
    let audio_bearer = crate::state::load_sharing_bearer_offload().await;
    let (Some(ts), Some(vp), Some(bearer)) = (audio_tailscale, audio_vocab_port, audio_bearer)
    else {
        return Ok(());
    };
    let audio_conn = crate::commands::sharing::PairedConnection {
        lan: None,
        tailscale: Some(ts),
        ports: medical_sharing::qr::PairPorts {
            ollama: 0,
            whisper: 0,
            pairing: 0,
            lmstudio: None,
            omlx: None,
            vocab: Some(vp),
        },
        label: String::new(),
    };
    let remote_for_audio =
        crate::content_remote::ContentRemote::from(&audio_conn, Some(bearer), Arc::clone(client));
    let Some(audio_remote) = remote_for_audio else {
        return Ok(());
    };

    // Resolve the recordings dir once (the per-recording writes below used
    // to re-resolve it inside every blocking task). Failure is non-fatal to
    // the round — it just skips the audio catch-up.
    let recordings_dir = {
        let db = Arc::clone(db);
        let data_dir_owned = data_dir.to_path_buf();
        let resolved = tokio::task::spawn_blocking(move || {
            crate::commands::resolve_recordings_dir(&db, &data_dir_owned)
        })
        .await
        .map_err(crate::commands::join_err);
        match resolved {
            Ok(Ok(dir)) => Some(dir),
            Ok(Err(ref e)) | Err(ref e) => {
                tracing::warn!(error = %e, "sync: recordings dir unavailable — skipping audio fetch (non-fatal)");
                None
            }
        }
    };
    let Some(recordings_dir) = recordings_dir else {
        return Ok(());
    };

    // Load the persisted skip set, pruning entries whose rows are no longer
    // empty-audio live rows, then select this cycle's candidates EXCLUDING
    // the skips. Without the exclusion the same oldest rows were reselected
    // forever, so a recording whose audio never arrives server-side
    // head-of-line blocked every later row from ever being attempted.
    let skips_and_ids = {
        let db = Arc::clone(db);
        tokio::task::spawn_blocking(move || -> AppResult<(AudioFetchSkips, Vec<String>)> {
            let conn = db.conn()?;
            let mut skips =
                ContentSyncRepo::get_audio_fetch_skips(&conn).map_err(AppError::from)?;
            skips = prune_audio_fetch_skips(&conn, skips)?;
            let skip_ids: Vec<String> = skips.iter().map(|(id, _)| id.clone()).collect();
            let ids = select_empty_audio_ids(&conn, &skip_ids)?;
            Ok((skips, ids))
        })
        .await
        .map_err(crate::commands::join_err)??
    };
    let (mut skips, missing_ids): (AudioFetchSkips, Vec<String>) = skips_and_ids;
    let mut skips_dirty = false;

    for rec_id in &missing_ids {
        // Active-row pre-check, mirroring the manual fetch command's gate: a
        // row trashed/purged after selection must not fetch (the guarded
        // audio-location write below would refuse it anyway). Failure of the
        // check itself is non-fatal.
        let row_active = {
            let db = Arc::clone(db);
            let rec_id_owned = rec_id.clone();
            let checked = tokio::task::spawn_blocking(move || -> AppResult<bool> {
                let conn = db.conn()?;
                let uuid = uuid::Uuid::parse_str(&rec_id_owned)
                    .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
                RecordingsRepo::get_by_id_active(&conn, &uuid)
                    .map(|_| true)
                    .or_else(|e| match e {
                        medical_db::DbError::NotFound(_) => Ok(false),
                        other => Err(AppError::from(other)),
                    })
            })
            .await
            .map_err(crate::commands::join_err);
            match checked {
                Ok(Ok(active)) => active,
                Ok(Err(ref e)) | Err(ref e) => {
                    tracing::warn!(error = %e, "sync: audio pre-check failed (non-fatal)");
                    false
                }
            }
        };
        if !row_active {
            // Row gone mid-cycle — its skip entry (if any) is useless now;
            // the prune would also catch it next cycle.
            skips_dirty |= clear_audio_fetch_skip(&mut skips, rec_id);
            continue;
        }

        match audio_remote.fetch_audio(rec_id).await {
            Ok(plaintext) => {
                let byte_count = plaintext.len();
                // Re-encrypt and save locally through the shared write
                // helper — the same orphan-cleanup contract as the manual
                // fetch command.
                let db2 = Arc::clone(db);
                let rec_id_owned = rec_id.clone();
                let recordings_dir_owned = recordings_dir.clone();
                match tokio::task::spawn_blocking(move || {
                    sync_loop_save_audio(&db2, &recordings_dir_owned, &rec_id_owned, &plaintext)
                })
                .await
                .map_err(crate::commands::join_err)
                {
                    Ok(_) => {
                        // Success clears any stale skip for the row.
                        skips_dirty |= clear_audio_fetch_skip(&mut skips, rec_id);
                        tracing::debug!(byte_count, "audio fetched and saved during sync");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "sync: failed to save fetched audio (non-fatal)");
                    }
                }
            }
            Err(e) if is_no_audio_on_server(&e) => {
                // The server holds the row but no audio for it yet. Skip the
                // row in future selections so later rows get their turn;
                // entries are pruned and capped, so this never becomes a
                // permanent block.
                bump_audio_fetch_skip(&mut skips, rec_id);
                skips_dirty = true;
                tracing::debug!(
                    recording_id = %rec_id,
                    "sync: no audio on the office server yet — row skipped for now"
                );
            }
            Err(e) => {
                tracing::debug!(error = %e, "sync: audio fetch failed (may not be available yet)");
            }
        }
    }
    if skips_dirty {
        let db = Arc::clone(db);
        let skips_for_persist = skips.clone();
        let result = tokio::task::spawn_blocking(move || -> AppResult<()> {
            let conn = db.conn()?;
            ContentSyncRepo::set_audio_fetch_skips(&conn, &skips_for_persist)
                .map_err(AppError::from)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "sync: persist audio fetch skips failed"),
            Err(e) => tracing::warn!(error = %e, "sync: persist audio fetch skips task failed"),
        }
    }
    Ok(())
}

/// Counts-only summary of a sync round (no PHI).
#[derive(Debug, Default, Clone, Copy)]
struct SyncSummary {
    pulled: usize,
    pushed: usize,
    merge_conflicts: usize,
    push_conflicts: usize,
}

/// Manually trigger a full bidirectional content sync.
///
/// Pulls server changes (per-field LWW merge into local), then pushes local
/// changes back. Emits a `content-sync-complete` Tauri event with a
/// counts-only payload (no PHI) when done so the frontend can refresh.
///
/// When not paired / sync disabled / no Tailscale, returns a zero summary
/// quietly (the app keeps working offline).
#[tauri::command]
#[instrument(skip(app, state), name = "content::sync_now")]
pub async fn sync_content_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<SyncSummaryPayload> {
    // Self-heal: backfill a missing Tailscale address before re-evaluating
    // the sync gate. Only runs when the paired connection doesn't already
    // have a Tailscale address, avoiding up to 5s latency per sync.
    if state::load_paired_connection_offload()
        .await
        .and_then(|c| c.tailscale)
        .is_none()
    {
        let _ = crate::commands::sharing::pairing::backfill_tailscale().await;
    }

    let Some((conn, bearer, http_client)) = content_sync_target(&state).await else {
        tracing::warn!(
            "content sync skipped: gates failed (see preceding debug logs for the reason)"
        );
        return Ok(SyncSummaryPayload {
            disabled: true,
            ..Default::default()
        });
    };
    let remote = match crate::content_remote::ContentRemote::from(&conn, Some(bearer), http_client)
    {
        Some(r) => r,
        None => {
            tracing::warn!("content sync skipped: transport setup failed after gates passed");
            return Ok(SyncSummaryPayload {
                disabled: true,
                ..Default::default()
            });
        }
    };
    // Serialize sync rounds to prevent cursor races (H3).
    let _guard = state.content_sync_lock.lock().await;
    let summary = run_sync(Arc::clone(&state.db), &state.data_dir, &remote, &app).await?;
    let payload = SyncSummaryPayload::from(summary);
    let _ = app.emit("content-sync-complete", payload);
    Ok(payload)
}

/// Startup initial sync — same logic as [`sync_content_now`] but takes
/// explicit params so it can run before `AppState` is registered with Tauri.
///
/// Called from `AppState::initialize` (or the app boot sequence) on startup.
/// Failures are logged but do not abort boot — the app must remain usable
/// offline. Emits `content-sync-complete` on the passed `AppHandle` if one is
/// available.
///
/// This is `pub` (not a `#[tauri::command]`) so it can be invoked directly
/// from `lib.rs::run`.
pub async fn run_initial_sync(app: tauri::AppHandle, db: Arc<Database>) {
    use tauri::Manager;

    // Acquire the sync lock to prevent racing with a user-triggered
    // sync_content_now. This is critical: without it, two concurrent sync
    // rounds could read the same cursor, double-merge, and interleave
    // writes at the SQLite level.
    let sync_lock = app
        .state::<crate::state::AppState>()
        .content_sync_lock
        .clone();
    let data_dir = app.state::<crate::state::AppState>().data_dir.clone();
    let _guard = sync_lock.lock().await;

    // Re-evaluate the gates without an AppState: load config + pairing from
    // disk directly. This mirrors content_sync_target but against raw state
    // helpers since AppState may not be fully wired yet at the call site.
    let config = crate::commands::settings::load_config_sync(&db).ok();
    let enabled = config.map(|c| c.sync_content).unwrap_or(false);
    if !enabled {
        return;
    }

    // Self-heal: if paired over LAN without a Tailscale address, probe the
    // server's /info endpoint to backfill it before the Tailscale gate below.
    // Runs on every startup so it retries until the server is reachable.
    let _ = crate::commands::sharing::pairing::backfill_tailscale().await;

    let Some(conn) = state::load_paired_connection_offload().await else {
        return;
    };
    if conn.ports.vocab.is_none() || conn.tailscale.is_none() {
        return;
    }
    let Some(bearer) = state::load_sharing_bearer_offload().await else {
        return;
    };
    let http_client = Arc::new(
        reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new()),
    );

    let remote = match crate::content_remote::ContentRemote::from(&conn, Some(bearer), http_client)
    {
        Some(r) => r,
        None => return,
    };
    match run_sync(db, &data_dir, &remote, &app).await {
        Ok(summary) => {
            tracing::info!(
                pulled = summary.pulled,
                pushed = summary.pushed,
                merge_conflicts = summary.merge_conflicts,
                push_conflicts = summary.push_conflicts,
                "initial content sync complete"
            );
            let _ = app.emit("content-sync-complete", SyncSummaryPayload::from(summary));
        }
        Err(e) => tracing::warn!(error = %e, "initial content sync failed (non-fatal)"),
    }
}

/// Counts-only payload emitted on `content-sync-complete` (no PHI).
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct SyncSummaryPayload {
    pub pulled: usize,
    pub pushed: usize,
    pub merge_conflicts: usize,
    pub push_conflicts: usize,
    /// True when the sync was skipped entirely because a gate failed
    /// (sync disabled, missing Tailscale address, unpaired, no token).
    /// Distinguishes "couldn't sync" from "synced, nothing changed" — the
    /// two were previously indistinguishable in the UI.
    pub disabled: bool,
}

impl From<SyncSummary> for SyncSummaryPayload {
    fn from(s: SyncSummary) -> Self {
        Self {
            pulled: s.pulled,
            pushed: s.pushed,
            merge_conflicts: s.merge_conflicts,
            push_conflicts: s.push_conflicts,
            // A real sync round is by definition not gate-disabled.
            disabled: false,
        }
    }
}

/// Start a long-lived SSE subscription to the office server's content-change
/// notifications.
///
/// Spawns a background task that connects to `/v1/content/events` and emits a
/// `content-changed` Tauri event for each server-pushed "changed"
/// notification. The frontend listens for this event and calls
/// `syncContentNow()` for near-realtime convergence across machines. The task
/// runs for the lifetime of the app and reconnects with exponential backoff
/// (5s → 30s cap) when the stream ends or errors.
///
/// Returns `Ok(())` immediately when not paired / sync disabled / no
/// Tailscale (no task is spawned). Safe to call repeatedly; each call spawns
/// an independent task. In practice the frontend calls it once on mount.
#[tauri::command]
#[instrument(skip(app, state), name = "content::subscribe")]
pub async fn subscribe_content_sync(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<()> {
    // When the gates fail (unpaired / sync disabled), cancel any existing
    // subscriber rather than leaving it reconnecting with stale credentials.
    // The probe result is otherwise unused — the spawned task re-resolves
    // the target (connection + fresh bearer) on EVERY reconnect.
    if content_sync_target(&state).await.is_none() {
        return crate::commands::swap_sse_cancel_token(
            &state.content_sse_cancel,
            "content_sse_cancel",
            None,
        );
    }

    // Cancel any existing SSE subscriber task before spawning a new one (H1).
    let cancel_token = tokio_util::sync::CancellationToken::new();
    crate::commands::swap_sse_cancel_token(
        &state.content_sse_cancel,
        "content_sse_cancel",
        Some(cancel_token.clone()),
    )?;

    let mut backoff = Duration::from_secs(5);
    let db_for_task = Arc::clone(&state.db);
    let http_for_task = state.http_client.clone();
    tokio::spawn(async move {
        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            // Re-evaluate the sync target on EVERY reconnect: pairing may
            // have changed since the last connection (re-pair issued a new
            // token, a revoke killed the old one, sync was disabled). The
            // subscribe_events_async contract requires exactly this — a
            // captured subscribe-time connection+bearer 401-loops with
            // dead credentials forever.
            let target = tokio::task::spawn_blocking({
                let db = Arc::clone(&db_for_task);
                let http_client = http_for_task.clone();
                move || content_sync_target_parts(&db, http_client)
            })
            .await
            .ok()
            .flatten();
            let Some((conn_owned, bearer, http_client)) = target else {
                // Unpaired / sync disabled / no token — nothing to
                // subscribe to anymore. Exit instead of looping.
                tracing::info!("content SSE: sync target no longer available; subscriber exiting");
                break;
            };
            let remote = match crate::content_remote::ContentRemote::from(
                &conn_owned,
                Some(bearer.clone()),
                http_client.clone(),
            ) {
                Some(r) => r,
                None => {
                    tracing::warn!("content SSE target unavailable, retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            match remote.subscribe_events_async().await {
                Ok(resp) => {
                    tracing::info!("content SSE subscription connected");
                    backoff = Duration::from_secs(5);
                    let mut stream = resp.bytes_stream();
                    // Buffer for incomplete SSE lines. TCP chunks can split a
                    // `data: changed\n` across two reads; without buffering,
                    // the split halves would never match and notifications
                    // would be silently dropped.
                    let mut sse_buffer = String::new();
                    loop {
                        // Cancellation must interrupt a healthy stream too —
                        // the server keep-alives the SSE connection
                        // indefinitely, so a reconnect-boundary check alone
                        // never fires.
                        tokio::select! {
                            _ = cancel_token.cancelled() => break,
                            chunk = stream.next() => {
                                let bytes = match chunk {
                                    Some(Ok(b)) => b,
                                    Some(Err(e)) => {
                                        tracing::warn!(
                                            error = %e,
                                            "content SSE chunk error"
                                        );
                                        break;
                                    }
                                    None => break,
                                };
                                sse_buffer.push_str(&String::from_utf8_lossy(&bytes));
                                // Normalize CRLF to LF so the \n\n split works
                                // regardless of whether intermediaries
                                // (proxies, Tailscale) upgrade to CRLF.
                                if sse_buffer.contains("\r\n") {
                                    sse_buffer = sse_buffer.replace("\r\n", "\n");
                                }
                                // SSE events are separated by blank lines
                                // (\n\n). Process only complete events.
                                while let Some(idx) = sse_buffer.find("\n\n") {
                                    let event = sse_buffer[..idx].to_string();
                                    sse_buffer = sse_buffer[idx + 2..].to_string();
                                    for line in event.lines() {
                                        if line.starts_with("data: changed") {
                                            let _ = app.emit("content-changed", ());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tracing::info!("content SSE stream ended, reconnecting");
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "content SSE subscription failed, reconnecting"
                ),
            }
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
    Ok(())
}

// ── Task 13: Audio commands ──────────────────────────────────────────────

/// Download audio for a recording from the office server, re-encrypt it
/// locally, write it to `{recordings_dir}/{id}.enc`, and update the DB
/// `audio_path`.
///
/// Used when a recording's metadata arrived via content sync but the audio
/// blob did not (audio is synced separately from field metadata). The server
/// returns decrypted plaintext bytes; this command re-encrypts them at rest
/// before the write completes so plaintext PHI never touches disk.
///
/// Refuses to fetch for a trashed/missing row (active-row pre-check), and
/// removes the written file if the row's `audio_path` update fails — the
/// row is gone or tombstoned, so nothing would reference the file and no
/// sweep cleans rowless `.enc` artifacts (both mirror the server-side PUT
/// handler's semantics).
///
/// Returns the local file path. No-op (returns the existing path) if the
/// audio is already present locally.
#[tauri::command]
#[instrument(skip(state), name = "content::fetch_audio")]
pub async fn fetch_audio_from_server(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    let (conn, bearer, http_client) = content_sync_target(&state)
        .await
        .ok_or_else(|| AppError::Other("content sync target unavailable".into()))?;
    let remote = crate::content_remote::ContentRemote::from(&conn, Some(bearer), http_client)
        .ok_or_else(|| AppError::Other("content remote unavailable (no tailscale?)".into()))?;

    // Resolve the local target path first so we can short-circuit if the
    // audio already exists (idempotent).
    let data_dir = state.data_dir.clone();
    let db = Arc::clone(&state.db);
    let recordings_dir = crate::commands::resolve_recordings_dir(&db, &data_dir)?;
    let target_path = recordings_dir.join(format!("{recording_id}.enc"));

    // First-write-wins: if we already have the audio, return its path.
    if target_path.exists() {
        return Ok(target_path.to_string_lossy().into_owned());
    }

    // Active-row pre-check, mirroring the server PUT handler's gate: a
    // trashed or missing row must not fetch audio. `update_audio_location`
    // below refuses tombstoned rows (`deleted_at IS NULL`), so without this
    // check a delete landing mid-fetch would strand the downloaded file
    // with no row pointing at it.
    {
        let db = Arc::clone(&state.db);
        let rec_id = recording_id.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let conn = db.conn()?;
            let uuid = uuid::Uuid::parse_str(&rec_id)
                .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
            RecordingsRepo::get_by_id_active(&conn, &uuid).map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(crate::commands::join_err)??;
    }

    // Download decrypted plaintext bytes from the server.
    let plaintext = remote.fetch_audio(&recording_id).await?;
    let byte_count = plaintext.len();

    let db2 = Arc::clone(&state.db);
    let target_for_task = target_path.clone();
    let rec_id_for_task = recording_id.clone();
    let path_str = tokio::task::spawn_blocking(move || {
        write_fetched_audio_locally(&db2, &target_for_task, &rec_id_for_task, &plaintext)
    })
    .await
    .map_err(crate::commands::join_err)??;

    tracing::debug!(
        recording_id_len = recording_id.len(),
        byte_count,
        "audio fetched and re-encrypted locally"
    );
    Ok(path_str)
}

/// Blocking core of [`fetch_audio_from_server`]: persist the fetched
/// plaintext as ciphertext at `target` (in-memory encrypt + atomic temp +
/// rename, so plaintext PHI never touches disk even on a mid-write crash),
/// then point the row's `audio_path` at it. Extracted from the command so
/// the failure-cleanup contract is unit-testable without a server.
fn write_fetched_audio_locally(
    db: &Arc<Database>,
    target: &std::path::Path,
    rec_id: &str,
    plaintext: &[u8],
) -> AppResult<String> {
    let byte_count = plaintext.len();
    let result: AppResult<String> = (|| {
        let tmp_path = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        medical_security::file_crypto::encrypt_file(&tmp_path, plaintext).map_err(|e| {
            // Clean up on failure — never leave PHI on disk.
            let _ = std::fs::remove_file(&tmp_path);
            AppError::security(format!("audio re-encrypt failed: {e}"))
        })?;
        if let Err(e) = std::fs::rename(&tmp_path, target) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AppError::Io(e));
        }

        // Update the recording's audio_path + file_size_bytes.
        // Audio-location-only write: must not bump `updated_at` (LWW stamp
        // inflation → silent loss of concurrent field edits).
        let conn = db.conn()?;
        let uuid = uuid::Uuid::parse_str(rec_id)
            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
        RecordingsRepo::update_audio_location(&conn, &uuid, target, Some(byte_count as u64))
            .map_err(AppError::from)?;
        Ok(target.to_string_lossy().into_owned())
    })();
    // Any failure AFTER the atomic rename removes the target: the file now
    // belongs to no row (`update_audio_location` refused because the row
    // was trashed/purged mid-fetch — its `deleted_at IS NULL` guard — or
    // the DB write itself failed) and NO sweep claims rowless `.enc`
    // artifacts, so keeping it would strand decryptable PHI forever.
    // Mirrors the server PUT handler's failure cleanup; a transient DB
    // error also lands here and the next fetch simply re-downloads.
    if result.is_err() {
        let _ = std::fs::remove_file(target);
    }
    result
}

/// Blocking write path for audio the SYNC LOOP fetched (see
/// [`sync_fetch_missing_audio`]). Rides [`write_fetched_audio_locally`] for
/// the fresh-write path so the orphan-cleanup contract (a row refusing the
/// guarded audio-location update mid-flight never strands the downloaded
/// ciphertext) is shared with the manual fetch command. The one
/// loop-specific addition: a file that already exists at the target is a
/// manual fetch racing the loop — point the row at it instead of
/// re-writing, since the row was selected for its empty `audio_path` and
/// would otherwise be reselected forever.
fn sync_loop_save_audio(
    db: &Arc<Database>,
    recordings_dir: &std::path::Path,
    rec_id: &str,
    plaintext: &[u8],
) -> AppResult<String> {
    let target = recordings_dir.join(format!("{rec_id}.enc"));
    if target.exists() {
        let conn = db.conn()?;
        let uuid = uuid::Uuid::parse_str(rec_id)
            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        // Audio-location-only write: must not bump `updated_at` (LWW stamp
        // inflation → silent loss of concurrent field edits). A refusal here
        // (row trashed mid-fetch) propagates — nothing was written, so
        // there is nothing to clean up.
        RecordingsRepo::update_audio_location(&conn, &uuid, &target, Some(size))
            .map_err(AppError::from)?;
        return Ok(target.to_string_lossy().into_owned());
    }
    write_fetched_audio_locally(db, &target, rec_id, plaintext)
}

/// Read local audio for a recording, decrypt it to plaintext, and upload it
/// to the office server.
///
/// The inverse of [`fetch_audio_from_server`]. Used when this machine created
/// the recording (so it owns the audio) and needs to push the blob to the
/// server so other paired clients can fetch it.
///
/// A server-side `409 Conflict` (the server already has this audio) is
/// treated as success — first-write-wins.
#[tauri::command]
#[instrument(skip(state), name = "content::upload_audio")]
pub async fn upload_audio_to_server(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<()> {
    let (conn, bearer, http_client) = content_sync_target(&state)
        .await
        .ok_or_else(|| AppError::Other("content sync target unavailable".into()))?;
    let remote = crate::content_remote::ContentRemote::from(&conn, Some(bearer), http_client)
        .ok_or_else(|| AppError::Other("content remote unavailable (no tailscale?)".into()))?;

    // Load the recording + decrypt its audio to plaintext on the blocking pool.
    let db = Arc::clone(&state.db);
    let rec_id_for_task = recording_id.clone();
    let plaintext = tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
        let conn = db.conn()?;
        let uuid = uuid::Uuid::parse_str(&rec_id_for_task)
            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
        let rec = RecordingsRepo::get_by_id(&conn, &uuid).map_err(AppError::from)?;
        let path = &rec.audio_path;
        if path.as_os_str().is_empty() || !path.exists() {
            return Err(AppError::Other("local audio file not found".into()));
        }
        match medical_security::file_crypto::decrypt_file(path) {
            Ok(plaintext) => Ok(plaintext),
            Err(medical_security::file_crypto::FileCryptoError::NotEncrypted) => {
                // Legacy plaintext file — read as-is.
                std::fs::read(path).map_err(|e| AppError::Other(format!("audio read failed: {e}")))
            }
            Err(e) => Err(AppError::security(format!("audio decrypt failed: {e}"))),
        }
    })
    .await
    .map_err(crate::commands::join_err)??;

    remote.upload_audio(&recording_id, plaintext).await
}

// Keep SYNCABLE_FIELDS referenced so the const stays part of the public
// surface even if the field list isn't directly used here yet. This also
// documents which fields participate in sync.
#[allow(dead_code)]
const _: &[&str] = SYNCABLE_FIELDS;

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::recording::Recording;
    use medical_db::Database;
    use medical_db::recordings::RecordingsRepo;

    #[test]
    fn merge_audio_queue_appends_without_duplicates() {
        let queue = vec!["old-a".to_string(), "old-b".to_string()];
        let merged = merge_audio_queue(&queue, &["new-c".into(), "old-a".into(), "new-d".into()]);
        assert_eq!(merged, vec!["old-a", "old-b", "new-c", "new-d"]);
        // Empty inputs stay empty.
        assert!(merge_audio_queue(&[], &[]).is_empty());
    }

    /// Happy path of the fetched-audio write: ciphertext on disk, row's
    /// `audio_path` pointing at it, no temp leftovers.
    #[test]
    fn write_fetched_audio_persists_ciphertext_and_points_the_row_at_it() {
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xCCu8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = {
            let conn = db.conn().expect("conn");
            let rec = Recording::new(
                "pulled.wav",
                std::path::PathBuf::from("/nonexistent/pulled.wav"),
            );
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            rec
        };
        let target = tmp.path().join(format!("{}.enc", rec.id));

        let out = write_fetched_audio_locally(&db, &target, &rec.id.to_string(), b"SERVER AUDIO")
            .expect("write succeeds for a live row");
        assert_eq!(out, target.to_string_lossy().into_owned());
        assert!(
            medical_security::file_crypto::is_encrypted(&target),
            "fetched audio must be ciphertext at rest"
        );
        {
            let conn = db.conn().expect("conn");
            let row = RecordingsRepo::get_by_id(&conn, &rec.id).expect("row");
            assert_eq!(row.audio_path, target);
            assert_eq!(row.file_size_bytes, Some(b"SERVER AUDIO".len() as u64));
        }
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read dir")
            .filter(|e| {
                e.as_ref()
                    .map(|e| e.file_name().to_string_lossy().contains(".tmp"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(leftovers.is_empty(), "no temp files may remain");
    }

    /// The orphan fix (2026-10-06 audio-delete review): when the row was
    /// trashed/purged while the fetch was in flight, the guarded
    /// `update_audio_location` refuses — and the just-written ciphertext
    /// must NOT be left behind (no sweep cleans rowless `.enc` files).
    #[test]
    fn write_fetched_audio_removes_target_when_row_refuses_the_update() {
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xCDu8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = {
            let conn = db.conn().expect("conn");
            let rec = Recording::new(
                "trashed.wav",
                std::path::PathBuf::from("/nonexistent/trashed.wav"),
            );
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            // Tombstone AFTER insert — the fetch's pre-check may have passed
            // moments earlier; this drives the mid-flight delete.
            RecordingsRepo::soft_delete(&conn, &rec.id).expect("soft delete");
            rec
        };
        let target = tmp.path().join(format!("{}.enc", rec.id));

        let result =
            write_fetched_audio_locally(&db, &target, &rec.id.to_string(), b"SERVER AUDIO");
        assert!(
            result.is_err(),
            "guarded update must refuse a tombstoned row"
        );
        assert!(
            !target.exists(),
            "the written ciphertext must be removed with the error — never orphaned"
        );
    }

    /// The LOOP-path mirror of the orphan fix above (2026-10-08 review,
    /// fix 5): the sync loop's fetched-audio write rides the same shared
    /// helper, so a row refusing the guarded update mid-flight must not
    /// strand the downloaded ciphertext there either.
    #[test]
    fn sync_loop_save_audio_removes_target_when_row_refuses_the_update() {
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xCEu8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = {
            let conn = db.conn().expect("conn");
            let rec = Recording::new(
                "loop-trashed.wav",
                std::path::PathBuf::from("/nonexistent/loop-trashed.wav"),
            );
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            // Tombstone AFTER insert — the loop's pre-download active-row
            // check may have passed moments earlier; this drives the
            // mid-flight delete reaching the write past the check.
            RecordingsRepo::soft_delete(&conn, &rec.id).expect("soft delete");
            rec
        };
        let target = tmp.path().join(format!("{}.enc", rec.id));

        let result = sync_loop_save_audio(&db, tmp.path(), &rec.id.to_string(), b"SERVER AUDIO");
        assert!(
            result.is_err(),
            "guarded update must refuse a tombstoned row on the loop path too"
        );
        assert!(
            !target.exists(),
            "the loop path must clean up the written ciphertext — never orphan it"
        );
    }

    /// Happy path of the loop-path write: ciphertext on disk, row pointed
    /// at it, and the already-exists race shortcut points the row at the
    /// existing file instead of re-writing it.
    #[test]
    fn sync_loop_save_audio_writes_ciphertext_and_handles_existing_target() {
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xCFu8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = {
            let conn = db.conn().expect("conn");
            let rec = Recording::new(
                "loop-live.wav",
                std::path::PathBuf::from("/nonexistent/loop-live.wav"),
            );
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            rec
        };

        // Fresh write.
        let out = sync_loop_save_audio(&db, tmp.path(), &rec.id.to_string(), b"LOOP AUDIO")
            .expect("write");
        assert_eq!(
            out,
            tmp.path().join(format!("{}.enc", rec.id)).to_string_lossy()
        );
        {
            let conn = db.conn().expect("conn");
            let row = RecordingsRepo::get_by_id(&conn, &rec.id).expect("row");
            assert_eq!(row.audio_path, tmp.path().join(format!("{}.enc", rec.id)));
        }

        // Already-exists race: a second save (as if the manual fetch command
        // won) re-points the row at the existing file without re-writing.
        let before = std::fs::read(tmp.path().join(format!("{}.enc", rec.id))).expect("ciphertext");
        let out = sync_loop_save_audio(&db, tmp.path(), &rec.id.to_string(), b"OTHER AUDIO")
            .expect("existing target short-circuits");
        assert_eq!(
            out,
            tmp.path().join(format!("{}.enc", rec.id)).to_string_lossy()
        );
        let after = std::fs::read(tmp.path().join(format!("{}.enc", rec.id))).expect("ciphertext");
        assert_eq!(
            before, after,
            "the existing ciphertext must not be re-written"
        );
    }

    /// The steady-state drain (2026-10-08 review, fix 3): queued upload
    /// retries used to fire only inside a non-empty push batch, so with no
    /// local changes they never ran. The extracted drain is what the
    /// empty-batch exit calls — it must attempt the queued id, drop it on
    /// success, and keep it on a transient upload failure.
    #[tokio::test]
    async fn drain_audio_uploads_attempts_queued_ids_without_a_push_batch() {
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xD1u8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = {
            let conn = db.conn().expect("conn");
            let wav = tmp.path().join("queued.wav");
            std::fs::write(&wav, b"RIFF....WAVEfmt ").expect("write wav");
            let rec = Recording::new("queued.wav", wav);
            RecordingsRepo::insert(&conn, &rec).expect("insert");
            rec
        };

        // A successful upload attempts once and empties the queue.
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_upload = std::sync::Arc::clone(&attempts);
        let mut queue = vec![rec.id.to_string()];
        drain_audio_uploads(&db, &mut queue, move |_id, _bytes| {
            let attempts = std::sync::Arc::clone(&attempts_for_upload);
            async move {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        })
        .await;
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the queued id must be attempted without any push batch"
        );
        assert!(queue.is_empty(), "a successful upload leaves the queue");

        // A failed upload keeps the id for the next round.
        let mut queue = vec![rec.id.to_string()];
        drain_audio_uploads(&db, &mut queue, move |_id, _bytes| async {
            Err(AppError::Other("upload failed".to_string()))
        })
        .await;
        assert_eq!(
            queue,
            vec![rec.id.to_string()],
            "a transient failure re-queues the id"
        );
    }

    /// The head-of-line fix (2026-10-08 review, fix 7): a 404-no-audio id is
    /// excluded from the next selection while a fresh empty-audio row is
    /// still selected; a successful fetch clears the entry so the row
    /// returns to selection; the skip set is capped by dropping the OLDEST
    /// entries.
    #[test]
    fn audio_fetch_selection_excludes_skips_until_cleared_and_caps_by_tenure() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let skipped = "00000000-0000-0000-0000-0000000000aa";
        let fresh = "00000000-0000-0000-0000-0000000000bb";
        for id in [skipped, fresh] {
            conn.execute(
                "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
                 VALUES (?1, ?2, '', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
                rusqlite::params![id, format!("{id}.wav")],
            )
            .expect("insert");
        }

        let mut skips: Vec<(String, u64)> = Vec::new();
        bump_audio_fetch_skip(&mut skips, skipped);
        let skip_ids: Vec<String> = skips.iter().map(|(id, _)| id.clone()).collect();
        let selected = select_empty_audio_ids(&conn, &skip_ids).expect("select with skip");
        assert!(
            !selected.contains(&skipped.to_string()),
            "the 404-no-audio id must be excluded from selection"
        );
        assert_eq!(
            selected,
            vec![fresh.to_string()],
            "a fresh empty-audio row is still selected"
        );

        // A successful fetch clears the entry — the row returns to selection.
        assert!(clear_audio_fetch_skip(&mut skips, skipped));
        assert!(skips.is_empty());
        let selected = select_empty_audio_ids(&conn, &[]).expect("select without skips");
        assert_eq!(
            selected.len(),
            2,
            "with the skip cleared both rows are selectable again"
        );

        // Repeated bumps increment in place; the cap drops the OLDEST entry.
        bump_audio_fetch_skip(&mut skips, skipped);
        assert_eq!(skips, vec![(skipped.to_string(), 1)]);
        bump_audio_fetch_skip(&mut skips, skipped);
        assert_eq!(skips, vec![(skipped.to_string(), 2)]);
        for i in 0..AUDIO_FETCH_SKIP_CAP {
            bump_audio_fetch_skip(&mut skips, &format!("cap-{i:03}"));
        }
        assert_eq!(skips.len(), AUDIO_FETCH_SKIP_CAP, "the skip set is capped");
        assert!(
            !skips.iter().any(|(id, _)| id == skipped),
            "the OLDEST entry (first bumped) is the one dropped at the cap"
        );
    }

    /// The skip set only ever records the typed "no audio on the server"
    /// 404 — transport/auth/5xx failures must retry next cycle instead of
    /// silently suppressing the row.
    #[test]
    fn is_no_audio_on_server_matches_only_the_typed_404_wording() {
        assert!(is_no_audio_on_server(&AppError::Other(
            "content audio fetch: no audio on the office server for this recording".into()
        )));
        assert!(!is_no_audio_on_server(&AppError::Other(
            "content audio fetch: HTTP 500".into()
        )));
        assert!(!is_no_audio_on_server(&AppError::Other(
            "content audio fetch: office server does not support content sync (update it to a later release)".into()
        )));
    }

    /// A skip must never outlive its usefulness: entries whose rows are no
    /// longer empty-audio live rows (audio arrived, row trashed/purged) are
    /// pruned at selection time.
    #[test]
    fn audio_fetch_skips_are_pruned_when_rows_leave_the_empty_audio_selection() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let fetched = "00000000-0000-0000-0000-0000000000cc";
        let trashed = "00000000-0000-0000-0000-0000000000dd";
        let still_missing = "00000000-0000-0000-0000-0000000000ee";
        for id in [fetched, trashed, still_missing] {
            conn.execute(
                "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
                 VALUES (?1, ?2, '', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
                rusqlite::params![id, format!("{id}.wav")],
            )
            .expect("insert");
        }
        // `fetched` got its audio via the manual command.
        conn.execute(
            "UPDATE recordings SET audio_path = '/audio/got-it.enc' WHERE id = ?1",
            [fetched],
        )
        .expect("fill audio_path");
        // `trashed` was tombstoned.
        conn.execute(
            "UPDATE recordings SET deleted_at = '2026-01-02T00:00:00+00:00' WHERE id = ?1",
            [trashed],
        )
        .expect("tombstone");

        let skips = vec![
            (fetched.to_string(), 2),
            (trashed.to_string(), 1),
            (still_missing.to_string(), 1),
        ];
        let pruned = prune_audio_fetch_skips(&conn, skips).expect("prune");
        assert_eq!(
            pruned,
            vec![(still_missing.to_string(), 1)],
            "only rows still empty-audio and live keep their skip entry"
        );

        // Empty input short-circuits.
        assert!(
            prune_audio_fetch_skips(&conn, Vec::new())
                .expect("prune empty")
                .is_empty()
        );
    }

    /// The retry queue's permanence classification: only live rows with a
    /// readable local audio file produce uploadable bytes — everything a
    /// cycle can never fix classifies `Gone` so it leaves the queue instead
    /// of retrying forever.
    #[test]
    fn read_local_audio_classifies_gone_versus_bytes() {
        // Keychain isolation: the RAII guard installs the global mock
        // provider (synthetic key), serialises against other
        // mock-installing tests, and clears it on drop — including on
        // panic. `encrypt_file`/`decrypt_bytes` route through
        // `keychain::get_or_create_db_key` and never reach the real OS
        // keychain. Pairing: the DB is in-memory and the fixtures live in
        // a tempdir, so the synthetic key only ever sees throwaway data.
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0xAAu8; 32]);

        let db = Arc::new(Database::open_in_memory().expect("db"));

        // The in-memory pool is max_size(1): all row setup happens in a
        // scoped connection that is dropped BEFORE any read_local_audio
        // call (which checks the pool out itself).
        let tmp = tempfile::tempdir().expect("tmp");
        let plaintext_wav = tmp.path().join("tombstoned.wav");
        std::fs::write(&plaintext_wav, b"RIFF....WAVEfmt ").expect("write wav");
        // Encrypted audio (the normal at-rest format). With the test
        // provider installed, `encrypt_file` uses the synthetic key and
        // succeeds deterministically — no OS keychain prompt, no fallback
        // to the keychain-free blob path.
        let encrypted_wav = tmp.path().join("encrypted.wav");
        medical_security::file_crypto::encrypt_file(&encrypted_wav, b"SECRET AUDIO")
            .expect("encrypt with synthetic key");

        // Malformed id → Gone (can never resolve).
        assert!(matches!(
            read_local_audio(&db, "not-a-uuid"),
            PendingAudioRead::Gone
        ));

        // No row at all → Gone.
        assert!(matches!(
            read_local_audio(&db, &uuid::Uuid::new_v4().to_string()),
            PendingAudioRead::Gone
        ));

        let (missing_id, tombstoned_id) = {
            let conn = db.conn().expect("conn");
            // Live row whose audio file is missing.
            let missing = Recording::new(
                "missing.wav",
                std::path::PathBuf::from("/nonexistent/missing.wav"),
            );
            RecordingsRepo::insert(&conn, &missing).expect("insert");
            // Tombstoned row with an EXISTING audio file.
            let tombstoned =
                Recording::new("tombstoned.wav", std::path::PathBuf::from(&plaintext_wav));
            RecordingsRepo::insert(&conn, &tombstoned).expect("insert");
            RecordingsRepo::soft_delete(&conn, &tombstoned.id).expect("soft delete");
            (missing.id, tombstoned.id)
        };

        assert!(matches!(
            read_local_audio(&db, &missing_id.to_string()),
            PendingAudioRead::Gone
        ));
        assert!(matches!(
            read_local_audio(&db, &tombstoned_id.to_string()),
            PendingAudioRead::Gone
        ));

        // Restored live row with a legacy PLAINTEXT wav → Bytes.
        {
            let conn = db.conn().expect("conn");
            RecordingsRepo::restore(&conn, &tombstoned_id).expect("restore");
        }
        match read_local_audio(&db, &tombstoned_id.to_string()) {
            PendingAudioRead::Bytes(b) => assert_eq!(b, b"RIFF....WAVEfmt "),
            other => panic!("expected Bytes, got {other:?}"),
        }

        // Encrypted audio (the normal at-rest format).
        {
            let conn = db.conn().expect("conn");
            let mut rec = RecordingsRepo::get_by_id(&conn, &tombstoned_id).expect("row");
            rec.audio_path = encrypted_wav;
            RecordingsRepo::update(&conn, &rec).expect("update path");
        }
        match read_local_audio(&db, &tombstoned_id.to_string()) {
            PendingAudioRead::Bytes(b) => {
                // With the synthetic test provider installed, the decrypt
                // round-trip is deterministic — both sides use the same key.
                assert_eq!(b, b"SECRET AUDIO");
            }
            other => panic!("expected Bytes from encrypted fixture, got {other:?}"),
        }
    }

    /// The composite cursor: the timestamp string is preserved verbatim
    /// (re-serializing would normalize `Z`→`+00:00` and break the exact
    /// string tie arm in `changed_since`) with the last-delivered id
    /// appended after `|`.
    #[test]
    fn advance_cursor_preserves_ts_and_appends_last_id() {
        let out = advance_cursor(
            "2026-01-02T03:04:05.123456Z",
            "00000000-0000-0000-0000-0000000000ff",
        );
        assert_eq!(
            out,
            "2026-01-02T03:04:05.123456Z|00000000-0000-0000-0000-0000000000ff"
        );
        // A `+00:00`-format timestamp round-trips identically.
        let out = advance_cursor("2026-01-02T03:04:05.123456+00:00", "aa");
        assert_eq!(out, "2026-01-02T03:04:05.123456+00:00|aa");
    }

    #[test]
    fn advance_cursor_passthrough_on_unparseable_input() {
        // An unparseable batch max is still a safe-enough cursor — the raw
        // value must come back unchanged (and bare, the legacy shape)
        // rather than empty or zeroed.
        assert_eq!(
            advance_cursor("not-a-timestamp", "some-id"),
            "not-a-timestamp"
        );
    }

    // Clock-skew clamp (2026-08-17 tracked item): a future-stamped batch max
    // must NOT advance the cursor past local now — that would pin every
    // fleet pull at the future instant and silently skip all present-day
    // writes until real time caught up. The clamped cursor keeps the id
    // component EMPTY (legacy shape): the bucket at that timestamp
    // re-delivers next pull, which the idempotent merges absorb.
    #[test]
    fn advance_cursor_clamps_future_timestamps_to_local_now() {
        let far_future = "2999-01-01T00:00:00Z";
        let out = advance_cursor(far_future, "00000000-0000-0000-0000-0000000000ff");
        assert!(
            !out.contains('|'),
            "a clamped cursor must carry no id component (got {out})"
        );
        let dt = chrono::DateTime::parse_from_rfc3339(&out).expect("clamped parses");
        let now = chrono::Utc::now();
        assert!(
            dt <= now && now.signed_duration_since(dt).num_seconds() < 60,
            "future cursor must clamp to ~local now, got {dt} (now {now})"
        );
        // Sanity: the clamp didn't happen via parse failure passthrough.
        assert_ne!(out, far_future);
    }

    /// The stranding bug this branch fixes (2026-10-08 review, fix 1):
    /// `soft_delete_all` stamps every row with one shared `updated_at`, and
    /// a batch limit of 200 used to strand the overflow tombstones forever
    /// (`julianday` cannot see a +1µs advance). Drive the actual
    /// selection+advance loop shape — changed_since → advance_cursor →
    /// repeat — and pin that all 205 same-timestamp rows travel.
    #[test]
    fn same_timestamp_rows_all_travel_across_repeated_batches() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        // 205 rows sharing one exact updated_at (the soft_delete_all shape).
        // Raw inserts, mirroring the db-side changed_since fixtures.
        let shared_ts = "2026-01-01T00:00:00+00:00";
        for i in 0..205 {
            conn.execute(
                "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, ?3)",
                rusqlite::params![
                    format!("00000000-0000-0000-{i:04}-000000000000"),
                    format!("bulk{i}.wav"),
                    shared_ts
                ],
            )
            .expect("insert");
        }

        // The pull/push loop shape: select a batch, compute the batch
        // boundary (max ts + max id among rows carrying it), advance the
        // cursor, repeat until nothing new comes back.
        let mut cursor = Some(advance_cursor("2025-12-31T00:00:00+00:00", ""));
        let mut delivered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(
                rounds < 10,
                "loop must terminate — same-timestamp rows are re-delivering forever"
            );
            let (ids, _has_more) =
                ContentSyncRepo::changed_since(&conn, cursor.as_deref(), 200).expect("select");
            if ids.is_empty() {
                break;
            }
            // What run_sync sees on the wire: each delivered row's
            // (updated_at, id), rebuilt from the row so the boundary math
            // is the production one over real payloads.
            let mut batch: Vec<SyncRecording> = Vec::with_capacity(ids.len());
            for id in &ids {
                let updated_at: String = conn
                    .query_row(
                        "SELECT updated_at FROM recordings WHERE id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .expect("row exists");
                batch.push(SyncRecording {
                    id: id.clone(),
                    filename: format!("{}.wav", id),
                    created_at: shared_ts.to_string(),
                    updated_at,
                    deleted_at: None,
                    patient_name: None,
                    duration_seconds: None,
                    file_size_bytes: None,
                    stt_provider: None,
                    ai_provider: None,
                    fields: HashMap::new(),
                });
            }
            for id in &ids {
                assert!(
                    delivered.insert(id.clone()),
                    "row {id} must not be re-delivered"
                );
            }
            let (ts, last_id) = batch_cursor_boundary(&batch).expect("non-empty batch boundary");
            cursor = Some(advance_cursor(&ts, &last_id));
        }

        assert_eq!(
            delivered.len(),
            205,
            "all same-timestamp rows must eventually travel; got {}",
            delivered.len()
        );
    }

    /// Seed a recording with two populated content fields plus a metadata
    /// blob carrying the local-only `synced_from` marker, and a revision row
    /// for the transcript.
    fn seed_recording(conn: &rusqlite::Connection) -> uuid::Uuid {
        let mut rec = Recording::new("visit.wav", std::path::PathBuf::from("/audio/visit.wav"));
        rec.transcript = Some("patient transcript text".to_string());
        rec.soap_note = Some("subjective objective assessment plan".to_string());
        rec.patient_name = Some("Doe".to_string());
        rec.metadata = serde_json::json!({
            "synced_from": "office-server-machine",
            "context": "freeform context"
        });
        // Row write OLDER than the revision below, so the revision wins the
        // max(revision, row) stamp and its assertions below hold. (The
        // opposite direction — newer row beating a stale revision — is
        // covered by build_sparse_fields_row_timestamp_wins_over_stale_revision.)
        rec.updated_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        RecordingsRepo::insert(conn, &rec).expect("insert recording");

        ContentSyncRepo::upsert_revision(
            conn,
            &rec.id,
            "transcript",
            "2026-06-01T10:00:00Z",
            Some("laptop-a"),
        )
        .expect("upsert transcript revision");
        rec.id
    }

    #[test]
    fn build_sync_recording_is_sparse_and_strips_synced_from() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let id = seed_recording(&conn);

        let sync = build_sync_recording(&conn, &id.to_string()).expect("build sync recording");

        // Sparse: populated fields present, absent fields omitted entirely.
        assert!(sync.fields.contains_key("transcript"));
        assert!(sync.fields.contains_key("soap_note"));
        assert!(sync.fields.contains_key("patient_name"));
        assert!(
            !sync.fields.contains_key("referral"),
            "absent fields must not participate in the merge"
        );

        // The revision row wins over the row-level timestamp, and carries the
        // origin device through to the wire payload.
        let transcript = &sync.fields["transcript"];
        assert_eq!(transcript.updated_at, "2026-06-01T10:00:00Z");
        assert_eq!(transcript.origin_device.as_deref(), Some("laptop-a"));

        // Fields without a revision fall back to the row-level timestamp.
        let soap = &sync.fields["soap_note"];
        assert_ne!(soap.updated_at, "2026-06-01T10:00:00Z");
        assert!(soap.origin_device.is_none());

        // The local-only synced_from marker must not round-trip to the origin
        // machine, but other metadata keys survive.
        let metadata = sync.fields["metadata"]
            .value
            .as_object()
            .expect("metadata obj");
        assert!(
            !metadata.contains_key("synced_from"),
            "synced_from must be stripped before push"
        );
        assert_eq!(metadata["context"], "freeform context");
    }

    #[test]
    fn build_sync_recording_rejects_invalid_id() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let err = build_sync_recording(&conn, "not-a-uuid").expect_err("must reject bad id");
        assert!(
            err.to_string().contains("invalid recording id"),
            "got: {err}"
        );
    }

    #[test]
    fn build_sparse_fields_row_timestamp_wins_over_stale_revision() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let mut rec = medical_core::types::recording::Recording::new(
            "rider.wav",
            std::path::PathBuf::from("/audio/rider.wav"),
        );
        rec.soap_note = Some("regenerated soap".to_string());
        let row_time = chrono::Utc::now();
        rec.updated_at = Some(row_time);
        RecordingsRepo::insert(&conn, &rec).expect("insert");
        // Stale revision from a pre-regeneration sync round-trip.
        ContentSyncRepo::upsert_revision(&conn, &rec.id, "soap_note", "2020-01-01T00:00:00Z", None)
            .expect("seed stale revision");

        let sync = build_sync_recording(&conn, &rec.id.to_string()).expect("build");
        let soap = &sync.fields["soap_note"];
        assert_ne!(
            soap.updated_at, "2020-01-01T00:00:00Z",
            "stale revision must not mask the newer row-level write"
        );
        assert_eq!(soap.updated_at, row_time.to_rfc3339());
        assert!(
            soap.origin_device.is_none(),
            "row-derived stamp carries no device"
        );

        // Newer revision still wins over the row.
        let newer_rev = (row_time + chrono::TimeDelta::seconds(60)).to_rfc3339();
        ContentSyncRepo::upsert_revision(&conn, &rec.id, "soap_note", &newer_rev, Some("desk-a"))
            .expect("seed newer revision");
        let sync2 = build_sync_recording(&conn, &rec.id.to_string()).expect("build 2");
        let soap2 = &sync2.fields["soap_note"];
        assert_eq!(soap2.updated_at, newer_rev, "newer revision wins");
        assert_eq!(soap2.origin_device.as_deref(), Some("desk-a"));
    }
}
