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

/// Advance a cursor timestamp by 1 microsecond past the batch boundary.
///
/// After a push/pull batch succeeds, the cursor is set to the batch's
/// `max(updated_at)`. Because `changed_since` uses strict `>` comparison,
/// two recordings sharing the same `updated_at` would silently lose the
/// second one (its timestamp is not `>` the cursor). Advancing the cursor
/// by 1 microsecond guarantees it is strictly greater than every timestamp
/// in the batch while still including same-timestamp recordings that were
/// not part of this batch.
///
/// Parses the RFC3339 timestamp, adds 1 microsecond, re-serializes. If the
/// input fails to parse, it is returned unchanged (the raw `max_ts` is
/// still a safe-enough cursor — the data-loss window only affects rows
/// sharing that exact timestamp).
fn advance_cursor(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => {
            let advanced = dt
                .checked_add_signed(chrono::Duration::microseconds(1))
                .unwrap_or(dt);
            advanced.to_rfc3339()
        }
        Err(_) => ts.to_string(),
    }
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
pub(crate) fn content_sync_target(
    state: &AppState,
) -> Option<(PairedConnection, String, Arc<reqwest::Client>)> {
    content_sync_target_parts(&state.db, state.http_client.clone())
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
/// For each syncable field that has content, look up its revision (if any)
/// to get the precise `updated_at` + `origin_device`; otherwise fall back to
/// the recording's row-level `updated_at`. Mirrors the server-side helper of
/// the same name.
fn build_sparse_fields(
    rec: &medical_core::types::recording::Recording,
    revisions: &[FieldRevision],
) -> HashMap<String, SyncFieldValue> {
    let mut fields: HashMap<String, SyncFieldValue> = HashMap::new();
    let rev_map: HashMap<&str, &FieldRevision> =
        revisions.iter().map(|r| (r.field.as_str(), r)).collect();

    let row_ts = rec
        .updated_at
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| rec.created_at.to_rfc3339());

    // Field wire timestamp = max(revision, row write). Writers that bump
    // only the row (transcription/generation completion via
    // `RecordingsRepo::update`) leave stale revisions from a pre-edit sync
    // round-trip; shipping the stale revision timestamp ties against the
    // server's copy and the merge's Equal arm silently drops the newer
    // value. Parsed comparison — string comparison is wrong across the two
    // stored timestamp formats. When the row is newer the origin device is
    // unknown (the row bump doesn't carry one).
    let field_ts = |rev: Option<&FieldRevision>| -> (String, Option<String>) {
        match rev {
            Some(r) => {
                if medical_db::content_sync::cmp_lww_timestamps(&r.updated_at, &row_ts)
                    == std::cmp::Ordering::Less
                {
                    (row_ts.clone(), None)
                } else {
                    (r.updated_at.clone(), r.origin_device.clone())
                }
            }
            None => (row_ts.clone(), None),
        }
    };

    let mut push_text = |name: &str, val: Option<&str>| {
        if let Some(s) = val {
            let (ts, device) = field_ts(rev_map.get(name).copied());
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: serde_json::Value::String(s.to_string()),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    push_text("transcript", rec.transcript.as_deref());
    push_text("soap_note", rec.soap_note.as_deref());
    push_text("referral", rec.referral.as_deref());
    push_text("letter", rec.letter.as_deref());
    push_text("peer_discussion", rec.peer_discussion.as_deref());
    push_text("chat", rec.chat.as_deref());
    push_text("patient_name", rec.patient_name.as_deref());

    let mut push_json = |name: &str, val: &serde_json::Value| {
        if !val.is_null() {
            let (ts, device) = field_ts(rev_map.get(name).copied());
            fields.insert(
                name.to_string(),
                SyncFieldValue {
                    value: val.clone(),
                    updated_at: ts,
                    origin_device: device,
                },
            );
        }
    };

    if let Ok(tags_json) = serde_json::to_value(&rec.tags) {
        push_json("tags", &tags_json);
    }
    push_json("metadata", &rec.metadata);
    let status_json = serde_json::to_value(&rec.status).unwrap_or(serde_json::Value::Null);
    push_json("processing_status", &status_json);

    fields
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

        // Merge incoming + advance the cursor. The next cursor is the max
        // `updated_at` in the batch (the server returns rows ordered by
        // updated_at ascending, so it's the last row's timestamp).
        let next_cursor = batch
            .recordings
            .iter()
            .map(|r| r.updated_at.as_str())
            .max()
            .map(|s| s.to_string());

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
        if let Some(ref nc) = next_cursor {
            let cursor_db = Arc::clone(&db);
            let nc = advance_cursor(nc);
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
    // without it (audio_path is empty). Best-effort: errors are logged
    // and don't abort the sync. Limit to 10 per cycle to bound latency.
    let audio_fetch_db = Arc::clone(&db);
    let audio_conn = crate::state::load_paired_connection();
    let audio_tailscale = audio_conn.as_ref().and_then(|c| c.tailscale.clone());
    let audio_vocab_port = audio_conn.as_ref().and_then(|c| c.ports.vocab);
    let audio_bearer = crate::state::load_sharing_bearer();
    if let (Some(ts), Some(vp), Some(bearer)) = (audio_tailscale, audio_vocab_port, audio_bearer) {
        let audio_conn = crate::commands::sharing::PairedConnection {
            lan: None,
            tailscale: Some(ts),
            ports: medical_sharing::qr::PairPorts {
                ollama: 0,
                whisper: 0,
                pairing: 0,
                lmstudio: None,
                vocab: Some(vp),
            },
            label: String::new(),
        };
        let remote_for_audio = crate::content_remote::ContentRemote::from(
            &audio_conn,
            Some(bearer),
            remote.client.clone(),
        );
        if let Some(audio_remote) = remote_for_audio {
            // Find recordings with empty audio_path (synced metadata, no audio yet).
            let missing_ids: Vec<String> = tokio::task::spawn_blocking({
                let db = Arc::clone(&audio_fetch_db);
                move || -> AppResult<Vec<String>> {
                    let conn = db.conn()?;
                    let mut stmt = conn.prepare(
                        "SELECT id FROM recordings WHERE audio_path = '' AND deleted_at IS NULL ORDER BY created_at ASC LIMIT 10",
                    ).map_err(|e| AppError::from(medical_db::DbError::from(e)))?;
                    let ids = stmt.query_map([], |row| row.get::<_, String>(0))
                        .map_err(|e| AppError::from(medical_db::DbError::from(e)))?
                        .filter_map(|r| r.ok())
                        .collect();
                    Ok(ids)
                }
            })
            .await
            .map_err(crate::commands::join_err)??;

            for rec_id in &missing_ids {
                match audio_remote.fetch_audio(rec_id).await {
                    Ok(plaintext) => {
                        let byte_count = plaintext.len();
                        // Re-encrypt and save locally.
                        let db2 = Arc::clone(&audio_fetch_db);
                        let rec_id_owned = rec_id.clone();
                        let plaintext_bytes = plaintext;
                        let data_dir_owned = data_dir.to_path_buf();
                        match tokio::task::spawn_blocking(move || -> AppResult<String> {
                            let conn = db2.conn()?;
                            let recordings_dir =
                                crate::commands::resolve_recordings_dir(&db2, &data_dir_owned)?;
                            let target = recordings_dir.join(format!("{rec_id_owned}.enc"));
                            if target.exists() {
                                // File already exists (race with manual fetch).
                                // Still update the DB audio_path since it was
                                // empty when we selected this row.
                                let uuid = uuid::Uuid::parse_str(&rec_id_owned).map_err(|e| {
                                    AppError::Other(format!("invalid recording id: {e}"))
                                })?;
                                if let Ok(mut rec) =
                                    medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                                {
                                    rec.audio_path = target.clone();
                                    rec.file_size_bytes = Some(
                                        std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0),
                                    );
                                    let _ =
                                        medical_db::recordings::RecordingsRepo::update(&conn, &rec);
                                }
                                return Ok(target.to_string_lossy().into_owned());
                            }
                            // Encrypt in memory before anything touches disk —
                            // a crash between write and encrypt would
                            // otherwise leave plaintext PHI in a .tmp file
                            // that no sweep cleans.
                            let tmp = target.with_extension("tmp");
                            medical_security::file_crypto::encrypt_file(&tmp, &plaintext_bytes)
                                .map_err(|e| {
                                    let _ = std::fs::remove_file(&tmp);
                                    AppError::security(format!("audio re-encrypt failed: {e}"))
                                })?;
                            std::fs::rename(&tmp, &target)?;
                            // Update DB.
                            let uuid = uuid::Uuid::parse_str(&rec_id_owned)
                                .map_err(|e| AppError::Other(format!("invalid id: {e}")))?;
                            let mut rec =
                                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                                    .map_err(AppError::from)?;
                            rec.audio_path = target.clone();
                            rec.file_size_bytes = Some(byte_count as u64);
                            medical_db::recordings::RecordingsRepo::update(&conn, &rec)
                                .map_err(AppError::from)?;
                            Ok(target.to_string_lossy().into_owned())
                        })
                        .await
                        .map_err(crate::commands::join_err)
                        {
                            Ok(_) => {
                                tracing::debug!(byte_count, "audio fetched and saved during sync");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "sync: failed to save fetched audio (non-fatal)");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "sync: audio fetch failed (may not be available yet)");
                    }
                }
            }
        }
    }

    // ── Push ────────────────────────────────────────────────────────────
    // Use a SEPARATE push cursor (independent from the pull cursor) so that
    // local recordings created before the first pull are still pushed.
    // The pull cursor tracks what we've received from the server; the push
    // cursor tracks what we've sent to the server. Without this separation,
    // the pull loop would advance the shared cursor past local recordings,
    // and they'd never be pushed.
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
            // If the batch is empty but there were IDs, we need the max
            // updated_at of those IDs so we can advance the push cursor past
            // them. Otherwise the push loop will livelock, retrying the same
            // unreadable recordings on every sync forever.
            let skip_cursor = if out.is_empty() && !ids.is_empty() {
                // Query the max updated_at of the IDs that failed to build.
                let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql =
                    format!("SELECT MAX(updated_at) FROM recordings WHERE id IN ({placeholders})");
                let params: Vec<&dyn rusqlite::ToSql> =
                    ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                conn.query_row(&sql, params.as_slice(), |row| {
                    row.get::<_, Option<String>>(0)
                })
                .ok()
                .flatten()
            } else {
                None
            };
            Ok::<(Vec<SyncRecording>, bool, Option<String>), AppError>((out, has_more, skip_cursor))
        })
        .await
        .map_err(crate::commands::join_err)??;

        let has_more = push_result.1;
        let batch = push_result.0;
        let skip_cursor = push_result.2;
        let batch_was_empty = batch.is_empty();

        if !batch_was_empty {
            let push_count = batch.len();
            // Capture recording IDs and max updated_at BEFORE moving batch.
            let pushed_ids: Vec<String> = batch.iter().map(|r| r.id.clone()).collect();
            let max_ts = batch
                .iter()
                .map(|r| r.updated_at.as_str())
                .max()
                .map(|s| s.to_string());
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
            if let Some(ts) = max_ts {
                let pc_db = Arc::clone(&db);
                let ts = advance_cursor(&ts);
                tokio::task::spawn_blocking(move || -> AppResult<()> {
                    let conn = pc_db.conn()?;
                    ContentSyncRepo::set_push_cursor(&conn, &ts).map_err(AppError::from)
                })
                .await
                .map_err(crate::commands::join_err)??;
            }
            // Best-effort audio upload for pushed recordings. Decrypts local
            // audio and uploads to server so the partner can fetch it.
            // Limit to 10 per cycle to bound latency. Errors are non-fatal.
            for rec_id in pushed_ids.iter().take(10) {
                let upload_db = Arc::clone(&db);
                let rec_id_owned = rec_id.clone();
                let plaintext_result =
                    tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
                        let conn = upload_db.conn()?;
                        let uuid = uuid::Uuid::parse_str(&rec_id_owned)
                            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
                        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                            .map_err(AppError::from)?;
                        let path = &rec.audio_path;
                        if path.as_os_str().is_empty() || !path.exists() {
                            return Err(AppError::Other("no local audio".into()));
                        }
                        match medical_security::file_crypto::decrypt_file(path) {
                            Ok(p) => Ok(p),
                            Err(medical_security::file_crypto::FileCryptoError::NotEncrypted) => {
                                std::fs::read(path)
                                    .map_err(|e| AppError::Other(format!("audio read failed: {e}")))
                            }
                            Err(e) => Err(AppError::security(format!("audio decrypt failed: {e}"))),
                        }
                    })
                    .await
                    .map_err(crate::commands::join_err);
                match plaintext_result {
                    Ok(Ok(plaintext)) => {
                        if let Err(e) = remote.upload_audio(rec_id, plaintext).await {
                            tracing::debug!(error = %e, "sync: audio upload failed");
                        }
                    }
                    Ok(Err(_)) | Err(_) => { /* no local audio — skip */ }
                }
            }
        } else if let Some(ts) = skip_cursor {
            // All recordings in this page were unreadable — advance the push
            // cursor past them so they're not retried on every sync.
            tracing::warn!(cursor = %ts, "content sync push: advancing cursor past unreadable recordings");
            let pc_db = Arc::clone(&db);
            let ts_owned = advance_cursor(&ts);
            tokio::task::spawn_blocking(move || -> AppResult<()> {
                let conn = pc_db.conn()?;
                ContentSyncRepo::set_push_cursor(&conn, &ts_owned).map_err(AppError::from)
            })
            .await
            .map_err(crate::commands::join_err)??;
        }

        if !has_more || batch_was_empty {
            break;
        }
    }

    Ok(summary)
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
    if state::load_paired_connection()
        .and_then(|c| c.tailscale)
        .is_none()
    {
        let _ = crate::commands::sharing::pairing::backfill_tailscale().await;
    }

    let Some((conn, bearer, http_client)) = content_sync_target(&state) else {
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

    let Some(conn) = state::load_paired_connection() else {
        return;
    };
    if conn.ports.vocab.is_none() || conn.tailscale.is_none() {
        return;
    }
    let Some(bearer) = state::load_sharing_bearer() else {
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
    let Some((conn, bearer, http_client)) = content_sync_target(&state) else {
        return crate::commands::swap_sse_cancel_token(
            &state.content_sse_cancel,
            "content_sse_cancel",
            None,
        );
    };

    // Cancel any existing SSE subscriber task before spawning a new one (H1).
    let cancel_token = tokio_util::sync::CancellationToken::new();
    crate::commands::swap_sse_cancel_token(
        &state.content_sse_cancel,
        "content_sse_cancel",
        Some(cancel_token.clone()),
    )?;

    let mut backoff = Duration::from_secs(5);
    let conn_owned = conn;
    tokio::spawn(async move {
        loop {
            if cancel_token.is_cancelled() {
                break;
            }
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
/// Returns the local file path. No-op (returns the existing path) if the
/// audio is already present locally.
#[tauri::command]
#[instrument(skip(state), name = "content::fetch_audio")]
pub async fn fetch_audio_from_server(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<String> {
    let (conn, bearer, http_client) = content_sync_target(&state)
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

    // Download decrypted plaintext bytes from the server.
    let plaintext = remote.fetch_audio(&recording_id).await?;
    let byte_count = plaintext.len();

    // Encrypt + write to disk + update DB audio_path, all on the blocking
    // pool. `encrypt_file` encrypts in memory and writes ciphertext
    // atomically (temp + rename), so plaintext PHI is never persisted even
    // if the process dies mid-write.
    let db2 = Arc::clone(&state.db);
    let target_for_task = target_path.clone();
    let rec_id_for_task = recording_id.clone();
    let path_str = tokio::task::spawn_blocking(move || -> AppResult<String> {
        let tmp_path =
            target_for_task.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        medical_security::file_crypto::encrypt_file(&tmp_path, &plaintext).map_err(|e| {
            // Clean up on failure — never leave PHI on disk.
            let _ = std::fs::remove_file(&tmp_path);
            AppError::security(format!("audio re-encrypt failed: {e}"))
        })?;
        if let Err(e) = std::fs::rename(&tmp_path, &target_for_task) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(AppError::Io(e));
        }

        // Update the recording's audio_path + file_size_bytes.
        let conn = db2.conn()?;
        let uuid = uuid::Uuid::parse_str(&rec_id_for_task)
            .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;
        let mut rec = RecordingsRepo::get_by_id(&conn, &uuid).map_err(AppError::from)?;
        rec.audio_path = target_for_task.clone();
        rec.file_size_bytes = Some(byte_count as u64);
        RecordingsRepo::update(&conn, &rec).map_err(AppError::from)?;

        Ok(target_for_task.to_string_lossy().into_owned())
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
    fn advance_cursor_adds_exactly_one_microsecond() {
        // Strict-`>` cursors lose same-timestamp rows; the +1µs advance is
        // what guarantees the second of two rows sharing max(updated_at) is
        // still picked up on the next pull.
        let out = advance_cursor("2026-01-02T03:04:05.123456Z");
        let dt = chrono::DateTime::parse_from_rfc3339(&out).expect("advanced parses");
        assert_eq!(dt.to_rfc3339(), "2026-01-02T03:04:05.123457+00:00");
        let orig = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123456Z")
            .expect("orig parses");
        assert_eq!(
            dt.signed_duration_since(orig).num_microseconds(),
            Some(1),
            "exactly one microsecond added"
        );
    }

    #[test]
    fn advance_cursor_passthrough_on_unparseable_input() {
        // An unparseable batch max is still a safe-enough cursor — the raw
        // value must come back unchanged rather than empty or zeroed.
        assert_eq!(advance_cursor("not-a-timestamp"), "not-a-timestamp");
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
