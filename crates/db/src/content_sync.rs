//! Content-sync repository: per-field last-write-wins merge for recordings.
//!
//! This module implements the wire-format types and the merge algorithm used
//! by the bidirectional content-sync layer. It mirrors the pattern of
//! [`crate::condition_chips`] but operates at the granularity of individual
//! text fields on a recording row (transcript, soap_note, referral, ...)
//! rather than whole rows.
//!
//! # Wire format
//!
//! [`SyncRecording`] is the transport type exchanged between machines. It
//! carries only sparse field data (`HashMap<String, SyncFieldValue>`) plus
//! enough recording metadata to insert a brand-new row. PHI-bearing text
//! never appears in this module's logs — see the HIPAA note below.
//!
//! # Merge algorithm
//!
//! [`ContentSyncRepo::merge_incoming`] compares each remote field revision
//! against the local one stored in `recording_field_revisions` and applies
//! the newer value, or records a [`MergeConflict`] when the local side wins.
//!
//! # HIPAA note
//!
//! No transcript, SOAP, referral, letter, or chat content is ever logged by
//! this module. Logging is restricted to counts, IDs, and field names.

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{DbError, DbResult};

/// Parse a stored timestamp for LWW decisions. Accepts both legitimate
/// stored formats: RFC 3339 (with any offset) and SQLite's space-separated
/// `datetime('now')` format. Returns `None` for anything else.
fn parse_lww_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Chronological comparison for LWW decisions. String comparison is wrong
/// across the two stored formats (`' '` < `T`, `Z` vs `+00:00`), so both
/// sides are parsed. Unparseable timestamps sort as the OLDEST value —
/// legacy or corrupt data must not win delete/restore decisions.
pub fn cmp_lww_timestamps(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_lww_timestamp(a), parse_lww_timestamp(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
    }
}

/// The set of recording fields that participate in per-field LWW sync.
///
/// Each name maps directly to a column on the `recordings` table. The
/// `apply_field` helper dispatches on these names when building UPDATE
/// statements.
pub const SYNCABLE_FIELDS: &[&str] = &[
    "transcript",
    "soap_note",
    "referral",
    "letter",
    "peer_discussion",
    "chat",
    "patient_name",
    "tags",
    "metadata",
    "processing_status",
];

/// A single field-level revision row.
///
/// One of these exists per `(recording_id, field)` pair in the
/// `recording_field_revisions` table. `updated_at` is an RFC 3339 string so
/// lexicographic ordering matches chronological ordering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldRevision {
    /// Name of the syncable field (one of [`SYNCABLE_FIELDS`]).
    pub field: String,
    /// When this field value was last written (RFC 3339 UTC string).
    pub updated_at: String,
    /// Machine that produced the write, if known.
    pub origin_device: Option<String>,
}

/// Persisted cursor state for incremental sync pulls.
///
/// `cursor` is the opaque server cursor (usually an `updated_at` watermark);
/// `last_pull` is the wall-clock time of the most recent successful pull, used
/// for diagnostics and backoff heuristics.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SyncCursor {
    /// Opaque cursor marking the position in the server's update stream.
    pub cursor: Option<String>,
    /// RFC 3339 timestamp of the last successful pull.
    pub last_pull: Option<String>,
}

/// Sparse field-value payload carried over the wire for one field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFieldValue {
    /// The field value as JSON (text fields use `Value::String`).
    pub value: serde_json::Value,
    /// When this value was last written (RFC 3339 UTC string).
    pub updated_at: String,
    /// Machine that produced the write, if known.
    pub origin_device: Option<String>,
}

/// Wire-format recording exchanged between machines during sync.
///
/// `fields` is sparse — only fields that the remote side has a value for are
/// present. Fields absent from the map are not considered for merge on this
/// round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRecording {
    /// Recording UUID as a string.
    pub id: String,
    /// Original audio filename.
    pub filename: String,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Whole-row modification timestamp (RFC 3339).
    pub updated_at: String,
    /// Soft-delete tombstone timestamp, if deleted.
    pub deleted_at: Option<String>,
    /// Optional patient name.
    pub patient_name: Option<String>,
    /// Audio duration in seconds.
    pub duration_seconds: Option<f64>,
    /// Audio file size in bytes.
    pub file_size_bytes: Option<u64>,
    /// STT provider name, if any.
    pub stt_provider: Option<String>,
    /// AI provider name, if any.
    pub ai_provider: Option<String>,
    /// Sparse field map (field name → value + timestamp).
    pub fields: HashMap<String, SyncFieldValue>,
}

/// A field where the local and remote revisions disagree and local won.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeConflict {
    /// The conflicting field name.
    pub field: String,
    /// Local revision timestamp (RFC 3339).
    pub local_updated_at: String,
    /// Remote revision timestamp (RFC 3339).
    pub remote_updated_at: String,
}

/// Outcome of merging a batch of remote recordings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergeResult {
    /// Fields where local won; caller may surface these to the user.
    pub conflicts: Vec<MergeConflict>,
    /// Local recording IDs whose data changed during this merge.
    pub changed_recording_ids: Vec<String>,
}

/// Repository for content-sync operations.
///
/// Stateles, associated-function style matching the other repos in this crate.
/// All methods take a `&Connection` as the first argument.
pub struct ContentSyncRepo;

impl ContentSyncRepo {
    /// Insert or update a single field-revision row.
    ///
    /// Uses `INSERT ... ON CONFLICT(recording_id, field) DO UPDATE` so the row
    /// is created on first write and the timestamp/device overwritten on
    /// subsequent writes.
    pub fn upsert_revision(
        conn: &Connection,
        recording_id: &uuid::Uuid,
        field: &str,
        updated_at: &str,
        origin_device: Option<&str>,
    ) -> DbResult<()> {
        conn.execute(
            "INSERT INTO recording_field_revisions
                (recording_id, field, updated_at, origin_device)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(recording_id, field) DO UPDATE SET
                updated_at    = excluded.updated_at,
                origin_device = excluded.origin_device",
            params![recording_id.to_string(), field, updated_at, origin_device,],
        )?;
        Ok(())
    }

    /// Load all field revisions for a single recording.
    pub fn revisions_for(
        conn: &Connection,
        recording_id: &uuid::Uuid,
    ) -> DbResult<Vec<FieldRevision>> {
        let mut stmt = conn.prepare(
            "SELECT field, updated_at, origin_device
             FROM recording_field_revisions
             WHERE recording_id = ?1",
        )?;
        let rows = stmt
            .query_map([recording_id.to_string()], |row| {
                Ok(FieldRevision {
                    field: row.get(0)?,
                    updated_at: row.get(1)?,
                    origin_device: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Bulk-load field revisions for many recordings in one query.
    ///
    /// Recordings with no revisions are simply absent from the result map.
    /// An empty input slice returns an empty map without touching the DB.
    pub fn revisions_for_batch(
        conn: &Connection,
        recording_ids: &[uuid::Uuid],
    ) -> DbResult<HashMap<String, Vec<FieldRevision>>> {
        let mut out: HashMap<String, Vec<FieldRevision>> = HashMap::new();
        if recording_ids.is_empty() {
            return Ok(out);
        }
        let placeholders = vec!["?"; recording_ids.len()].join(",");
        let sql = format!(
            "SELECT recording_id, field, updated_at, origin_device
             FROM recording_field_revisions
             WHERE recording_id IN ({placeholders})"
        );
        let id_strings: Vec<String> = recording_ids.iter().map(|u| u.to_string()).collect();
        let params: Vec<&dyn rusqlite::ToSql> = id_strings
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            let recording_id: String = row.get(0)?;
            Ok((
                recording_id,
                FieldRevision {
                    field: row.get(1)?,
                    updated_at: row.get(2)?,
                    origin_device: row.get(3)?,
                },
            ))
        })?;
        for r in rows {
            let (recording_id, revision) = r?;
            out.entry(recording_id).or_default().push(revision);
        }
        Ok(out)
    }

    /// Read the persisted sync cursor from the `sync_state` table.
    ///
    /// Returns a default (empty) cursor if the keys are NULL (first run).
    /// Propagates actual DB errors rather than masking them as "first run,"
    /// which would cause silent full re-pulls on transient failures.
    pub fn get_cursor(conn: &Connection) -> DbResult<SyncCursor> {
        // The migration seeds rows with NULL values. We need to handle
        // both "row not found" (shouldn't happen post-migration) and
        // "row exists but value is NULL" (first run). Using `row.get`
        // on a NULL column returns InvalidColumnType, so we use
        // `row.get::<_, Option<String>>` which maps NULL → None.
        //
        // QueryResultNoRows (row missing entirely) is treated as first run.
        // Other errors (SQLITE_BUSY, disk I/O, corruption) are propagated.
        let cursor: Option<String> = match conn.query_row(
            "SELECT value FROM sync_state WHERE key = 'content_sync_cursor'",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(DbError::from(e)),
        };
        let last_pull: Option<String> = match conn.query_row(
            "SELECT value FROM sync_state WHERE key = 'content_sync_last_pull'",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(DbError::from(e)),
        };
        Ok(SyncCursor { cursor, last_pull })
    }

    /// Persist the sync cursor and stamp `last_pull` to now (UTC RFC 3339).
    pub fn set_cursor(conn: &Connection, cursor: Option<&str>) -> DbResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE sync_state SET value = ?1 WHERE key = 'content_sync_cursor'",
            params![cursor],
        )?;
        conn.execute(
            "UPDATE sync_state SET value = ?1 WHERE key = 'content_sync_last_pull'",
            params![now],
        )?;
        Ok(())
    }

    /// Read the push cursor — a separate watermark tracking the newest
    /// `updated_at` of recordings that have been successfully pushed to the
    /// server. Independent from the pull cursor so that:
    ///   - Pulling server recordings doesn't mark local recordings as "pushed"
    ///   - Local recordings created before the first pull still get pushed
    ///
    /// Returns `None` on first run (nothing pushed yet → push everything).
    ///
    /// Includes a one-time reset: if `content_sync_push_v3_reset` is not set,
    /// the push cursor is cleared (set to NULL) so that the first push after
    /// upgrading to the separate-push-cursor version sends ALL local
    /// recordings. Without this, recordings created before the push cursor
    /// was introduced would never sync.
    pub fn get_push_cursor(conn: &Connection) -> DbResult<Option<String>> {
        // One-time reset: clear any stale push cursor left by earlier
        // versions that conflated pull and push cursors. Uses a versioned
        // sentinel key so each version bump can trigger a fresh full re-push.
        let needs_reset: bool = match conn.query_row(
            "SELECT COUNT(*) FROM sync_state WHERE key = 'content_sync_push_v3_reset'",
            [],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(0) => true,
            Ok(_) => false,
            // Row should always exist (COUNT always returns a row), but if
            // something goes wrong, skip the reset rather than failing.
            Err(_) => false,
        };
        if needs_reset {
            conn.execute(
                "INSERT OR REPLACE INTO sync_state (key, value) VALUES ('content_sync_push_v3_reset', '1')",
                [],
            )?;
            conn.execute(
                "UPDATE sync_state SET value = NULL WHERE key = 'content_sync_push_cursor'",
                [],
            )?;
            tracing::info!("push cursor reset (one-time v3 migration): forcing full re-push");
            return Ok(None);
        }

        let cursor: Option<String> = match conn.query_row(
            "SELECT value FROM sync_state WHERE key = 'content_sync_push_cursor'",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(DbError::from(e)),
        };
        Ok(cursor)
    }

    /// Persist the push cursor after a successful push batch.
    pub fn set_push_cursor(conn: &Connection, cursor: &str) -> DbResult<()> {
        // INSERT OR IGNORE ensures the row exists; UPDATE sets the value.
        conn.execute(
            "INSERT OR IGNORE INTO sync_state (key, value) VALUES ('content_sync_push_cursor', NULL)",
            [],
        )?;
        conn.execute(
            "UPDATE sync_state SET value = ?1 WHERE key = 'content_sync_push_cursor'",
            params![cursor],
        )?;
        Ok(())
    }

    /// Delta query: return recording IDs modified since the given cursor.
    ///
    /// `since` is an RFC 3339 `updated_at` watermark; `None` returns
    /// everything (used for the initial pull). Results are ordered by
    /// `updated_at` ascending and capped at `limit`. The boolean in the
    /// returned tuple is `true` when more rows are available.
    pub fn changed_since(
        conn: &Connection,
        since: Option<&str>,
        limit: u32,
    ) -> DbResult<(Vec<String>, bool)> {
        let limit_i64 = limit as i64;
        // Fetch limit+1 rows to detect "has_more" without a separate COUNT.
        let fetch = limit_i64 + 1;
        let ids: Vec<String> = if let Some(since) = since {
            let mut stmt = conn.prepare(
                "SELECT id FROM recordings
                 WHERE updated_at > ?1
                 ORDER BY updated_at ASC
                 LIMIT ?2",
            )?;
            stmt.query_map(params![since, fetch], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id FROM recordings
                 ORDER BY updated_at ASC
                 LIMIT ?1",
            )?;
            stmt.query_map(params![fetch], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let has_more = ids.len() as i64 > limit_i64;
        let mut out = ids;
        if has_more {
            out.truncate(limit_i64 as usize);
        }
        Ok((out, has_more))
    }

    // -----------------------------------------------------------------
    // Merge
    // -----------------------------------------------------------------

    /// Merge a batch of remote recordings into the local store using
    /// per-field last-write-wins.
    ///
    /// See the module docs for the wire format. The whole batch runs inside a
    /// single transaction; a failure rolls back all writes so the local store
    /// is never left half-merged.
    ///
    /// Returns the list of conflicts (local won) and the IDs of recordings
    /// whose local data changed.
    pub fn merge_incoming(conn: &Connection, remotes: &[SyncRecording]) -> DbResult<MergeResult> {
        let mut conflicts: Vec<MergeConflict> = Vec::new();
        let mut changed: Vec<String> = Vec::new();

        // Whole batch inside one transaction; a failure rolls back all writes
        // so the local store is never left half-merged. `unchecked_transaction`
        // rolls back on drop, and `Transaction` derefs to `Connection` so the
        // merge body below uses `conn` unchanged.
        let tx = conn.unchecked_transaction()?;
        let conn: &Connection = &tx;
        for remote in remotes {
            let id_str = &remote.id;
            let id = match uuid::Uuid::parse_str(id_str) {
                Ok(u) => u,
                Err(e) => {
                    return Err(DbError::UuidParse(
                        e.to_string(),
                        format!("SyncRecording.id = {id_str}"),
                    ));
                }
            };

            let local_exists = local_recording_exists(conn, id_str)?;

            // ----- Deletion handling (whole-row tombstone) -----
            if let Some(remote_deleted) = &remote.deleted_at {
                if local_exists {
                    let (local_deleted, local_updated): (Option<String>, Option<String>) =
                        match conn.query_row(
                            "SELECT deleted_at, updated_at FROM recordings WHERE id = ?1",
                            [id_str],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        ) {
                            Ok(v) => v,
                            // Unreachable in practice (`local_exists` was
                            // checked inside this same transaction) — treat as
                            // nothing-to-do rather than failing the whole batch.
                            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                            Err(e) => return Err(DbError::from(e)),
                        };
                    match local_deleted {
                        None => {
                            // Local live, remote tombstoned → timestamped LWW.
                            // A remote deletion at or after the local edit
                            // tombstones the row (ties go to the tombstone —
                            // deletions win ties); a strictly newer local edit
                            // keeps the row live and will be pushed back to the
                            // deleting peer. A NULL local `updated_at` sorts
                            // oldest, so a legacy row never outlives a real
                            // deletion.
                            if cmp_lww_timestamps(
                                remote_deleted,
                                local_updated.as_deref().unwrap_or(""),
                            ) != std::cmp::Ordering::Less
                            {
                                Self::sync_tombstone(conn, id_str, remote_deleted)?;
                                changed.push(id_str.clone());
                                tracing::info!(
                                    recording_id = %id_str,
                                    "sync: applied remote tombstone (deletion at or after local edit)"
                                );
                            } else {
                                tracing::debug!(
                                    recording_id = %id_str,
                                    "sync: local edit newer than remote tombstone; keeping row live"
                                );
                            }
                        }
                        Some(local_ts) => {
                            // Both deleted — nothing to reconcile; the local
                            // tombstone always stands (even when the remote
                            // one is later). Any UPDATE here would fire the
                            // FTS update trigger's 'delete' against absent
                            // index state (tombstoned rows are de-indexed
                            // from `recordings_fts` → SQLITE_CORRUPT), so
                            // this is deliberately a conservative no-op:
                            // keeping the local timestamp just means purge
                            // waits a little longer, and peers converge —
                            // any peer holding the other tombstone keeps it
                            // by the same rule.
                            if cmp_lww_timestamps(remote_deleted, &local_ts)
                                == std::cmp::Ordering::Less
                            {
                                tracing::debug!(
                                    recording_id = %id_str,
                                    "sync: both sides deleted; peer holds an earlier tombstone (FTS-safe no-op)"
                                );
                            }
                        }
                    }
                    // Once deleted, skip field-level merge for this recording.
                    continue;
                } else {
                    // Remote is a tombstone for a recording we don't have —
                    // insert it as a tombstone so the deletion is durable,
                    // unless the purge ledger already records this id: the
                    // ledger row itself makes the deletion durable, and
                    // re-inserting would resurrect a purged recording's row.
                    if Self::purge_ledger_refuses(conn, id_str) {
                        tracing::warn!(
                            recording_id = %id_str,
                            "sync: refused tombstone insert of purged recording"
                        );
                        continue;
                    }
                    Self::insert_remote_recording(conn, remote)?;
                    changed.push(id_str.clone());
                    continue;
                }
            }

            // ----- New recording (no local row) → insert + all revisions -----
            if !local_exists {
                if Self::purge_ledger_refuses(conn, id_str) {
                    tracing::warn!(recording_id = %id_str,
                        "sync: refused re-insert of purged recording (stale copy from a machine that missed the deletion)");
                    continue;
                }
                Self::insert_remote_recording(conn, remote)?;
                for (field, value) in &remote.fields {
                    Self::apply_field(conn, id_str, field, &value.value)?;
                    Self::upsert_revision(
                        conn,
                        &id,
                        field,
                        &value.updated_at,
                        value.origin_device.as_deref(),
                    )?;
                }
                // Stamp the synced_from marker AFTER the field merge loop,
                // so it survives even if the sender's metadata field
                // overwrites the initial value set by insert_remote_recording.
                // Non-fatal: the badge is cosmetic and must never block
                // the data sync from completing.
                if let Err(e) = Self::stamp_synced_origin(conn, id_str, remote) {
                    tracing::warn!(
                        recording_id = %id_str,
                        error = %e,
                        "sync: stamp_synced_origin failed (non-fatal, badge will be missing)"
                    );
                }
                changed.push(id_str.clone());
                continue;
            }

            // ----- Existing recording → restore check, then per-field LWW -----
            // A local tombstone guards every field write (`apply_field`'s
            // `deleted_at IS NULL`), so a live incoming row must first beat
            // the tombstone's timestamp to matter: a strictly newer live row
            // means a peer restored the recording — revive it (FTS-safe) and
            // fall through so the field loop below can apply the peer's
            // edits. An older live row is a pre-delete copy — the tombstone
            // stands and the row is skipped entirely (fields stay guarded,
            // and we don't emit the apply_field 0-rows warning per field).
            let local_deleted: Option<String> = match conn.query_row(
                "SELECT deleted_at FROM recordings WHERE id = ?1",
                [id_str],
                |r| r.get(0),
            ) {
                Ok(v) => v,
                // Defensive: `local_exists` was checked inside this same
                // transaction, so a missing row here is a vanishing edge —
                // skip it rather than failing the whole batch. Real DB errors
                // (I/O, busy) are propagated; swallowing them would silently
                // skip a legitimate restore while the cursor advances.
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(e) => return Err(DbError::from(e)),
            };
            if let Some(local_del) = &local_deleted {
                if cmp_lww_timestamps(&remote.updated_at, local_del) == std::cmp::Ordering::Greater
                {
                    Self::sync_restore(conn, id_str, &remote.updated_at)?;
                    changed.push(id_str.clone());
                    tracing::info!(
                        recording_id = %id_str,
                        "sync: remote live row newer than local tombstone; restored"
                    );
                    // Fall through: the field loop is now unblocked.
                } else {
                    tracing::debug!(
                        recording_id = %id_str,
                        "sync: local tombstone newer than remote live row; staying deleted"
                    );
                    continue;
                }
            }

            let local_revisions = Self::revisions_for(conn, &id)?;
            let local_map: HashMap<&str, &FieldRevision> = local_revisions
                .iter()
                .map(|r| (r.field.as_str(), r))
                .collect();

            let mut row_changed = false;
            for (field, remote_value) in &remote.fields {
                match local_map.get(field.as_str()) {
                    None => {
                        // No local revision → remote wins by default.
                        Self::apply_field(conn, id_str, field, &remote_value.value)?;
                        Self::upsert_revision(
                            conn,
                            &id,
                            field,
                            &remote_value.updated_at,
                            remote_value.origin_device.as_deref(),
                        )?;
                        row_changed = true;
                    }
                    Some(local_rev) => {
                        match remote_value.updated_at.cmp(&local_rev.updated_at) {
                            std::cmp::Ordering::Greater => {
                                // Remote newer → remote wins.
                                Self::apply_field(conn, id_str, field, &remote_value.value)?;
                                Self::upsert_revision(
                                    conn,
                                    &id,
                                    field,
                                    &remote_value.updated_at,
                                    remote_value.origin_device.as_deref(),
                                )?;
                                row_changed = true;
                            }
                            std::cmp::Ordering::Less => {
                                // Local newer → local wins, report conflict.
                                conflicts.push(MergeConflict {
                                    field: field.clone(),
                                    local_updated_at: local_rev.updated_at.clone(),
                                    remote_updated_at: remote_value.updated_at.clone(),
                                });
                            }
                            std::cmp::Ordering::Equal => {
                                // Tie — keep local, no conflict.
                            }
                        }
                    }
                }
            }
            if row_changed {
                // Bump the row's updated_at so the next delta pull sees it.
                // `deleted_at IS NULL` keeps this an FTS-safe no-op on
                // locally-trashed rows (a local tombstone wins over peer
                // field edits — see `apply_field`).
                let max_ts = remote
                    .fields
                    .values()
                    .map(|v| v.updated_at.as_str())
                    .max()
                    .unwrap_or(&remote.updated_at);
                conn.execute(
                    "UPDATE recordings SET updated_at = ?1
                         WHERE id = ?2 AND deleted_at IS NULL AND ?1 > updated_at",
                    params![max_ts, id_str],
                )?;
                changed.push(id_str.clone());
            }
        }

        tx.commit()?;

        tracing::info!(
            remote_count = remotes.len(),
            conflict_count = conflicts.len(),
            changed_count = changed.len(),
            "sync merge complete"
        );

        Ok(MergeResult {
            conflicts,
            changed_recording_ids: changed,
        })
    }

    /// True when the purge ledger records ANY row for this id.
    ///
    /// Id-only by design: a machine that was offline across the deletion can
    /// EDIT its stale copy, giving it a fresh `updated_at` that would pierce
    /// a `purged_at >= updated_at` comparison. Genuinely re-created content
    /// always gets a NEW UUID, so same-UUID + a ledger hit is always a stale
    /// copy of a recording the practice deleted and the server purged.
    ///
    /// COUNT always returns exactly one row, so the `unwrap_or(0)` below only
    /// fires on a genuinely broken ledger read — which falls OPEN (allow the
    /// insert): data availability wins over blocking sync on a ledger hiccup.
    fn purge_ledger_refuses(conn: &Connection, id: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM purged_recordings WHERE id = ?1",
            [id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    /// Write a single field value to the `recordings` table.
    ///
    /// Dispatches on the field name to build a targeted UPDATE. Text fields
    /// accept a JSON string; `tags`, `metadata`, and `processing_status`
    /// store the JSON value directly (their columns are JSON text). Unknown
    /// field names are a no-op (logged) so a newer server field doesn't
    /// break an older client.
    ///
    /// The UPDATE is guarded with `deleted_at IS NULL`: a local tombstone
    /// wins over a peer's field edit (the recording is deleted locally, so
    /// the skipped edit is correct), and — critically — an UPDATE on a
    /// soft-deleted row would fire the `recordings_fts_update` trigger
    /// against a de-indexed row (SQLITE_CORRUPT). The guard makes the edit
    /// a clean no-op; the sync cursor still advances.
    fn apply_field(
        conn: &Connection,
        recording_id: &str,
        field: &str,
        value: &serde_json::Value,
    ) -> DbResult<()> {
        // For text columns the wire payload is a JSON string; extract it.
        // For JSON columns (tags, metadata, processing_status) store the
        // serialized JSON as-is.
        let text_value: Option<String> = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };

        // Validate processing_status before writing — a malformed wire value
        // would be stored verbatim and silently deserialized to Pending by
        // row_to_recording, downgrading a Completed recording with no log.
        if field == "processing_status"
            && let Some(ref val_str) = text_value
        {
            let parse_result: Result<medical_core::types::recording::ProcessingStatus, _> =
                serde_json::from_str(val_str);
            if parse_result.is_err() {
                tracing::warn!(
                    field = %field,
                    "sync: apply_field received invalid processing_status value, skipping to avoid silent downgrade to Pending"
                );
                return Ok(());
            }
        }

        let column = match field {
            "transcript" | "soap_note" | "referral" | "letter" | "peer_discussion" | "chat"
            | "patient_name" => field,
            "tags" | "metadata" | "processing_status" => field,
            other => {
                tracing::warn!(field = %other, "sync: ignoring unknown field");
                return Ok(());
            }
        };

        let sql =
            format!("UPDATE recordings SET {column} = ?1 WHERE id = ?2 AND deleted_at IS NULL");
        let changed = conn.execute(&sql, params![text_value, recording_id])?;
        if changed == 0 {
            tracing::warn!(
                recording_id = %recording_id,
                field = %field,
                "sync: apply_field updated 0 rows (recording missing, or locally trashed — tombstone wins)"
            );
        }
        Ok(())
    }

    /// Insert a brand-new recording row from a `SyncRecording`.
    ///
    /// Used when a recording arrives that doesn't exist locally. Audio-path
    /// columns are set to safe defaults since the audio file is synced
    /// separately (the recordings table row must exist before field
    /// revisions can reference it).
    fn insert_remote_recording(conn: &Connection, remote: &SyncRecording) -> DbResult<()> {
        let tags_json = serde_json::json!([]).to_string();
        // Use the remote's actual processing_status if present and valid,
        // instead of a hardcoded "pending" that could silently downgrade
        // a Completed recording.
        let status_json = remote
            .fields
            .get("processing_status")
            .map(|v| v.value.to_string())
            .filter(|s| {
                serde_json::from_str::<medical_core::types::recording::ProcessingStatus>(s).is_ok()
            })
            .unwrap_or_else(|| serde_json::json!({"status": "pending"}).to_string());
        // Stamp metadata with a synced_from marker so the receiving machine
        // can display a "remote" badge. Uses the first field revision's
        // origin_device if available, otherwise "remote".
        let origin = remote
            .fields
            .values()
            .find_map(|v| v.origin_device.clone())
            .unwrap_or_else(|| "remote".to_string());
        let metadata_json = serde_json::json!({"synced_from": origin}).to_string();
        conn.execute(
            "INSERT INTO recordings (
                id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                patient_name, audio_path, duration_seconds, file_size_bytes,
                stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                updated_at, deleted_at
             ) VALUES (
                ?1, ?2, NULL, NULL, NULL, NULL, NULL, NULL,
                ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14
             )",
            params![
                remote.id,
                remote.filename,
                remote.patient_name,
                "", // audio_path placeholder — real audio synced separately
                remote.duration_seconds,
                remote.file_size_bytes.map(|n| n as i64),
                remote.stt_provider,
                remote.ai_provider,
                tags_json,
                status_json,
                remote.created_at,
                metadata_json,
                remote.updated_at,
                remote.deleted_at,
            ],
        )?;

        // The INSERT above fires `recordings_fts_insert` unconditionally, so
        // a row inserted AS a tombstone (the insert-as-tombstone path in
        // `merge_incoming`) would otherwise sit indexed while tombstoned. A
        // later `sync_restore` would re-index the already-indexed row and
        // leave a duplicate posting — a single FTS 'delete' then removes
        // only one copy. De-index immediately with the just-inserted values
        // (exactly what the trigger indexed), mirroring `sync_tombstone`'s
        // guarded de-index. Non-fatal to match its discipline: an FTS hiccup
        // must not fail the whole sync batch.
        if remote.deleted_at.is_some()
            && let Err(e) = conn.execute(
                "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
                 SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
                 FROM recordings WHERE id = ?1",
                [&remote.id],
            )
        {
            tracing::warn!(
                recording_id = %remote.id,
                error = %e,
                "insert_remote_recording: failed to de-index inserted tombstone row"
            );
        }
        Ok(())
    }

    /// Merge a `synced_from` key into the recording's metadata JSON after the
    /// per-field merge loop has applied the sender's metadata. This ensures
    /// the origin stamp survives even when the sender's metadata field
    /// overwrites the placeholder set by `insert_remote_recording`.
    fn stamp_synced_origin(conn: &Connection, id: &str, remote: &SyncRecording) -> DbResult<()> {
        // Read the current metadata (may have been updated by apply_field).
        let current: String = conn.query_row(
            "SELECT metadata FROM recordings WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )?;
        let mut meta: serde_json::Value =
            serde_json::from_str(&current).unwrap_or(serde_json::Value::Null);
        if meta.is_null() {
            meta = serde_json::json!({});
        }
        // Ensure metadata is a JSON object. If it's not (e.g. a string or
        // number from a malformed peer), wrap it to preserve the original
        // value rather than discarding it.
        if !meta.is_object() {
            meta = serde_json::json!({ "original": meta });
        }
        // After the above guards, meta is guaranteed to be a JSON object.
        let obj = meta
            .as_object_mut()
            .expect("metadata is object after guards");
        if !obj.contains_key("synced_from") {
            let origin = remote
                .fields
                .values()
                .find_map(|v| v.origin_device.clone())
                .unwrap_or_else(|| "remote".to_string());
            obj.insert("synced_from".to_string(), serde_json::json!(origin));
        }
        let updated = meta.to_string();
        conn.execute(
            "UPDATE recordings SET metadata = ?1 WHERE id = ?2",
            params![updated, id],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Sync-driven tombstone / restore primitives
    // -----------------------------------------------------------------

    /// Tombstone a live local row from a sync peer's deletion, with a
    /// caller-supplied timestamp (the deletion's own `deleted_at`). Mirrors
    /// `RecordingsRepo::soft_delete`'s FTS discipline: UPDATE first, then
    /// remove the FTS row with the *currently indexed* column values.
    /// Missing row / already-tombstoned → clean no-op.
    pub fn sync_tombstone(conn: &Connection, id: &str, deleted_at: &str) -> DbResult<()> {
        let changed = conn.execute(
            "UPDATE recordings SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![deleted_at, id],
        )?;
        // Only de-index when the UPDATE actually tombstoned the row. Unlike
        // `soft_delete` (which errors out on 0 rows and can therefore assume
        // it runs on live rows), the no-op case here must NOT fire the FTS
        // 'delete': the row was never indexed (missing) or is already
        // de-indexed (tombstoned), and a 'delete' against absent index state
        // corrupts the external-content index (SQLITE_CORRUPT on the next
        // FTS operation).
        if changed > 0
            && let Err(e) = conn.execute(
                "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
                 SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
                 FROM recordings WHERE id = ?1",
                [id],
            )
        {
            tracing::warn!(error = %e, "sync_tombstone: failed to remove recording from FTS index");
        }
        Ok(())
    }

    /// Revive a tombstoned local row from a sync peer's newer restore, with
    /// the restore's `updated_at`. Mirrors `RecordingsRepo::restore`'s FTS
    /// discipline: re-index BEFORE the UPDATE (the update trigger fires a
    /// 'delete' for the old values that must match indexed state). Does NOT
    /// stamp `retention_exempt` — the origin machine's restore stamped it
    /// and it travels in the synced `metadata` field. Missing / live row →
    /// clean no-op.
    pub fn sync_restore(conn: &Connection, id: &str, updated_at: &str) -> DbResult<()> {
        // EXISTS always returns exactly one row, so any error here is a
        // genuine DB failure — propagate it. Swallowing it as "not
        // tombstoned" (.unwrap_or(false)) would silently skip the restore
        // while the sync cursor advances, permanently losing the peer's
        // restore.
        let tombstoned: bool = match conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL)",
            [id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            // Theoretically unreachable (EXISTS always yields a row); treat
            // it as not-tombstoned rather than failing the sync batch.
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => return Err(DbError::from(e)),
        };
        if !tombstoned {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO recordings_fts(rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1",
            [id],
        )?;
        let changed = conn.execute(
            "UPDATE recordings SET deleted_at = NULL, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NOT NULL",
            params![updated_at, id],
        )?;
        // Close the probe→UPDATE TOCTOU. If the row flipped to live or was
        // purged between the EXISTS probe and this UPDATE (0 rows changed),
        // the re-insert above either added a duplicate index entry (row went
        // live: it was already indexed) or inserted nothing (row gone).
        // Remove exactly one entry with the mirror 'delete' so a concurrent
        // flip can't leave a duplicate; it is a no-op when nothing was
        // inserted.
        if changed == 0
            && let Err(e) = conn.execute(
                "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
                 SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
                 FROM recordings WHERE id = ?1",
                [id],
            )
        {
            tracing::warn!(error = %e, "sync_restore: failed to roll back stray FTS re-index");
        }
        Ok(())
    }
}

/// Return `true` if a recording with the given id exists locally (regardless
/// of soft-delete state). Looks only at presence, not content.
fn local_recording_exists(conn: &Connection, id: &str) -> DbResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recordings WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[test]
    fn cursor_round_trips_through_sync_state() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        // Default cursor is empty on a fresh DB.
        let initial = ContentSyncRepo::get_cursor(&conn).expect("get");
        assert!(initial.cursor.is_none());
        assert!(initial.last_pull.is_none());

        ContentSyncRepo::set_cursor(&conn, Some("2026-07-10T00:00:00Z")).expect("set");

        let after = ContentSyncRepo::get_cursor(&conn).expect("get");
        assert_eq!(after.cursor.as_deref(), Some("2026-07-10T00:00:00Z"));
        assert!(after.last_pull.is_some(), "last_pull should be stamped");
    }

    #[test]
    fn changed_since_returns_all_when_no_cursor() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        // Seed two recordings with distinct updated_at timestamps.
        for i in 0..2 {
            conn.execute(
                "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, ?3)",
                params![
                    format!("00000000-0000-0000-0000-00000000000{i}"),
                    format!("f{i}.wav"),
                    format!("2026-07-0{i}T10:00:00Z"),
                ],
            )
            .expect("insert");
        }

        let (ids, has_more) = ContentSyncRepo::changed_since(&conn, None, 100).expect("query");
        assert_eq!(ids.len(), 2);
        assert!(!has_more);

        let (ids, has_more) = ContentSyncRepo::changed_since(&conn, None, 1).expect("query");
        assert_eq!(ids.len(), 1);
        assert!(has_more, "fetching fewer than total should report has_more");
    }

    #[test]
    fn changed_since_filters_by_cursor() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        conn.execute(
            "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
             VALUES ('a', 'a.wav', '', '2026-07-01T00:00:00Z', '2026-07-01T00:00:00Z')",
            [],
        )
        .expect("insert old");
        conn.execute(
            "INSERT INTO recordings (id, filename, audio_path, created_at, updated_at)
             VALUES ('b', 'b.wav', '', '2026-07-02T00:00:00Z', '2026-07-02T00:00:00Z')",
            [],
        )
        .expect("insert new");

        let (ids, has_more) =
            ContentSyncRepo::changed_since(&conn, Some("2026-07-01T00:00:00Z"), 100)
                .expect("query");
        assert_eq!(ids, vec!["b".to_string()]);
        assert!(!has_more);
    }

    #[test]
    fn revisions_for_batch_handles_empty_input() {
        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");
        let map = ContentSyncRepo::revisions_for_batch(&conn, &[]).expect("batch");
        assert!(map.is_empty());
    }
}

#[cfg(test)]
mod lww_ts_tests {
    use super::cmp_lww_timestamps;
    use std::cmp::Ordering;

    #[test]
    fn rfc3339_offsets_compare_chronologically() {
        // String comparison would order these wrongly (Z vs +00:00).
        assert_eq!(
            cmp_lww_timestamps("2026-01-02T03:04:05Z", "2026-01-02T03:04:05+00:00"),
            Ordering::Equal
        );
        assert_eq!(
            cmp_lww_timestamps("2026-01-02T03:04:05.500Z", "2026-01-02T03:04:05Z"),
            Ordering::Greater
        );
    }

    #[test]
    fn legacy_space_format_compares_chronologically() {
        // ' ' (0x20) < 'T' (0x54): string comparison puts the LATER
        // space-format timestamp before the earlier RFC one on the same day.
        assert_eq!(
            cmp_lww_timestamps("2026-01-02 05:00:00", "2026-01-02T03:04:05Z"),
            Ordering::Greater
        );
        assert_eq!(
            cmp_lww_timestamps("2026-01-02 01:00:00", "2026-01-02T03:04:05Z"),
            Ordering::Less
        );
    }

    #[test]
    fn unparseable_sorts_oldest() {
        assert_eq!(
            cmp_lww_timestamps("garbage", "2026-01-02T03:04:05Z"),
            Ordering::Less
        );
        assert_eq!(
            cmp_lww_timestamps("2026-01-02T03:04:05Z", "garbage"),
            Ordering::Greater
        );
        assert_eq!(cmp_lww_timestamps("", ""), Ordering::Equal);
    }
}

/// Tests for the FTS-disciplined sync tombstone/restore helpers.
///
/// **HIPAA note:** assertions touch only ids, counts, and timestamps;
/// fixture filenames are synthetic.
#[cfg(test)]
mod sync_tombstone_tests {
    use super::*;
    use crate::Database;
    use crate::recordings::RecordingsRepo;
    use medical_core::types::recording::Recording;

    fn seed(conn: &Connection, filename: &str) -> Recording {
        let rec = Recording::new(
            filename,
            std::path::PathBuf::from(format!("/audio/{filename}")),
        );
        RecordingsRepo::insert(conn, &rec).expect("seed");
        rec
    }

    fn deleted_at_raw(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT deleted_at FROM recordings WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("query deleted_at")
    }

    fn updated_at_raw(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT updated_at FROM recordings WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .expect("query updated_at")
    }

    /// `recordings_fts` is an external-content FTS5 table
    /// (`content='recordings'`): plain SELECTs read column values from the
    /// content table, so `WHERE id = ?` always "finds" the row and never
    /// observes de-indexing. Index membership must be probed with a MATCH
    /// on a token unique to the fixture — its filename stem.
    fn fts_row_present(conn: &Connection, filename_stem: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM recordings_fts WHERE recordings_fts MATCH ?1",
            [format!("filename:{filename_stem}")],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    /// Assert the external-content FTS index is internally consistent.
    /// A 'delete' issued against absent/mismatched index state corrupts the
    /// index; this fails loudly instead of letting it surface later as
    /// SQLITE_CORRUPT on an unrelated query.
    fn assert_fts_healthy(conn: &Connection) {
        conn.execute(
            "INSERT INTO recordings_fts(recordings_fts) VALUES('integrity-check')",
            [],
        )
        .expect("FTS integrity-check must pass");
    }

    #[test]
    fn sync_tombstone_hides_row_and_deindexes_fts() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        let rec = seed(&conn, "tqz.wav");
        assert!(fts_row_present(&conn, "tqz"));
        ContentSyncRepo::sync_tombstone(&conn, &rec.id.to_string(), "2026-06-01T00:00:00Z")
            .unwrap();
        assert_eq!(
            deleted_at_raw(&conn, &rec.id.to_string()).as_deref(),
            Some("2026-06-01T00:00:00Z")
        );
        assert!(
            !fts_row_present(&conn, "tqz"),
            "tombstoned row must leave the FTS index"
        );
        assert_fts_healthy(&conn);
    }

    #[test]
    fn sync_restore_revives_row_and_reindexes_fts() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        let rec = seed(&conn, "revx.wav");
        assert!(fts_row_present(&conn, "revx"));
        RecordingsRepo::soft_delete(&conn, &rec.id).unwrap();
        assert!(!fts_row_present(&conn, "revx"));
        ContentSyncRepo::sync_restore(&conn, &rec.id.to_string(), "2026-06-02T00:00:00Z").unwrap();
        assert_eq!(deleted_at_raw(&conn, &rec.id.to_string()), None);
        assert_eq!(
            updated_at_raw(&conn, &rec.id.to_string()).as_deref(),
            Some("2026-06-02T00:00:00Z"),
            "sync_restore must stamp updated_at with the caller-supplied value"
        );
        assert!(
            fts_row_present(&conn, "revx"),
            "restored row must be searchable again"
        );
        assert_fts_healthy(&conn);
    }

    #[test]
    fn sync_tombstone_on_missing_or_deleted_row_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        ContentSyncRepo::sync_tombstone(
            &conn,
            "00000000-0000-0000-0000-000000000000",
            "2026-06-01T00:00:00Z",
        )
        .unwrap();
        let rec = seed(&conn, "noopn.wav");
        RecordingsRepo::soft_delete(&conn, &rec.id).unwrap();
        // second tombstone with a different timestamp is a no-op (row already
        // deleted). Critically it must not re-fire the FTS 'delete' against
        // already-de-indexed state (index corruption).
        ContentSyncRepo::sync_tombstone(&conn, &rec.id.to_string(), "2027-01-01T00:00:00Z")
            .unwrap();
        assert_ne!(
            deleted_at_raw(&conn, &rec.id.to_string()).as_deref(),
            Some("2027-01-01T00:00:00Z")
        );
        assert!(
            !fts_row_present(&conn, "noopn"),
            "double tombstone must not resurrect the row"
        );
        assert_fts_healthy(&conn);
        // exercise the index further: restore + search round-trip
        ContentSyncRepo::sync_restore(&conn, &rec.id.to_string(), "2027-02-02T00:00:00Z").unwrap();
        assert!(
            fts_row_present(&conn, "noopn"),
            "restore after double tombstone must re-index"
        );
        assert_eq!(
            updated_at_raw(&conn, &rec.id.to_string()).as_deref(),
            Some("2027-02-02T00:00:00Z")
        );
        assert_fts_healthy(&conn);
    }

    #[test]
    fn sync_restore_on_missing_or_live_row_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        ContentSyncRepo::sync_restore(
            &conn,
            "00000000-0000-0000-0000-000000000000",
            "2026-06-02T00:00:00Z",
        )
        .unwrap();
        let rec = seed(&conn, "livel.wav");
        let before = deleted_at_raw(&conn, &rec.id.to_string());
        let before_updated = updated_at_raw(&conn, &rec.id.to_string());
        ContentSyncRepo::sync_restore(&conn, &rec.id.to_string(), "2026-06-02T00:00:00Z").unwrap();
        assert_eq!(deleted_at_raw(&conn, &rec.id.to_string()), before);
        assert_eq!(
            updated_at_raw(&conn, &rec.id.to_string()),
            before_updated,
            "no-op restore must not bump updated_at (would cause spurious delta re-sync)"
        );
        assert!(
            fts_row_present(&conn, "livel"),
            "live row keeps its FTS entry"
        );
        assert_fts_healthy(&conn);
    }
}
