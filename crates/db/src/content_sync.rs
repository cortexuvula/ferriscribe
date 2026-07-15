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
        let cursor: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'content_sync_cursor'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
        let last_pull: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'content_sync_last_pull'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
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
    /// Includes a one-time reset: if `content_sync_push_v2_reset` is not set,
    /// the push cursor is cleared (set to NULL) so that the first push after
    /// upgrading to the separate-push-cursor version sends ALL local
    /// recordings. Without this, recordings created before the push cursor
    /// was introduced would never sync.
    pub fn get_push_cursor(conn: &Connection) -> DbResult<Option<String>> {
        // One-time reset: clear any stale push cursor left by earlier
        // versions that conflated pull and push cursors.
        let needs_reset: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE key = 'content_sync_push_v2_reset'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0;
        if needs_reset {
            conn.execute(
                "INSERT OR REPLACE INTO sync_state (key, value) VALUES ('content_sync_push_v2_reset', '1')",
                [],
            )?;
            conn.execute(
                "UPDATE sync_state SET value = NULL WHERE key = 'content_sync_push_cursor'",
                [],
            )?;
            tracing::info!("push cursor reset (one-time v2 migration): forcing full re-push");
            return Ok(None);
        }

        let cursor: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'content_sync_push_cursor'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
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

        conn.execute_batch("BEGIN")?;
        let result: DbResult<()> = (|| {
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
                        let local_deleted: Option<String> = conn
                            .query_row(
                                "SELECT deleted_at FROM recordings WHERE id = ?1",
                                [id_str],
                                |row| row.get(0),
                            )
                            .ok()
                            .flatten();
                        match local_deleted {
                            None => {
                                // Local live, remote deleted → propagate deletion.
                                conn.execute(
                                    "UPDATE recordings SET deleted_at = ?1, updated_at = ?1
                                     WHERE id = ?2",
                                    params![remote_deleted, id_str],
                                )?;
                                changed.push(id_str.clone());
                                tracing::info!(
                                    recording_id = %id_str,
                                    deleted_at = %remote_deleted,
                                    "sync: propagated remote deletion"
                                );
                            }
                            Some(local_ts) => {
                                // Both deleted — earliest (smallest timestamp) wins.
                                if remote_deleted < &local_ts {
                                    conn.execute(
                                        "UPDATE recordings SET deleted_at = ?1, updated_at = ?1
                                         WHERE id = ?2",
                                        params![remote_deleted, id_str],
                                    )?;
                                    changed.push(id_str.clone());
                                }
                            }
                        }
                        // Once deleted, skip field-level merge for this recording.
                        continue;
                    } else {
                        // Remote is a tombstone for a recording we don't have —
                        // insert it as a tombstone so the deletion is durable.
                        Self::insert_remote_recording(conn, remote)?;
                        changed.push(id_str.clone());
                        continue;
                    }
                }

                // ----- New recording (no local row) → insert + all revisions -----
                if !local_exists {
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
                    changed.push(id_str.clone());
                    continue;
                }

                // ----- Existing recording → per-field LWW -----
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
                    let max_ts = remote
                        .fields
                        .values()
                        .map(|v| v.updated_at.as_str())
                        .max()
                        .unwrap_or(&remote.updated_at);
                    conn.execute(
                        "UPDATE recordings SET updated_at = ?1 WHERE id = ?2 AND ?1 > updated_at",
                        params![max_ts, id_str],
                    )?;
                    changed.push(id_str.clone());
                }
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

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

    /// Write a single field value to the `recordings` table.
    ///
    /// Dispatches on the field name to build a targeted UPDATE. Text fields
    /// accept a JSON string; `tags`, `metadata`, and `processing_status`
    /// store the JSON value directly (their columns are JSON text). Unknown
    /// field names are a no-op (logged) so a newer server field doesn't
    /// break an older client.
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

        let column = match field {
            "transcript" | "soap_note" | "referral" | "letter" | "peer_discussion" | "chat"
            | "patient_name" => field,
            "tags" | "metadata" | "processing_status" => field,
            other => {
                tracing::warn!(field = %other, "sync: ignoring unknown field");
                return Ok(());
            }
        };

        let sql = format!("UPDATE recordings SET {column} = ?1 WHERE id = ?2");
        let changed = conn.execute(&sql, params![text_value, recording_id])?;
        if changed == 0 {
            tracing::warn!(
                recording_id = %recording_id,
                field = %field,
                "sync: apply_field updated 0 rows"
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
        let status_json = serde_json::json!({"status": "pending"}).to_string();
        let metadata_json = serde_json::Value::Null.to_string();
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
