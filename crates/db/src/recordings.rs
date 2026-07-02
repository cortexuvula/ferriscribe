//! CRUD operations for the `recordings` table.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};
use uuid::Uuid;

use medical_core::types::recording::{ProcessingStatus, Recording, RecordingSummary};

use crate::{DbError, DbResult};

/// Repository for the `recordings` table -- the central entity of the app.
///
/// All methods are associated functions that take a `&Connection`. The table
/// stores audio metadata, transcripts, SOAP notes, referrals, letters, and
/// a JSON `metadata` column (see crate-level docs for the dual-field design).
pub struct RecordingsRepo;

impl RecordingsRepo {
    /// Insert a new recording.  All JSON fields are serialised before storing.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] on constraint
    /// violation (e.g. duplicate ID) or serialisation failure.
    pub fn insert(conn: &Connection, recording: &Recording) -> DbResult<()> {
        let status_json = serde_json::to_string(&recording.status)
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let tags_json = serde_json::to_string(&recording.tags)
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let metadata_json = recording.metadata.to_string();

        conn.execute(
            "INSERT INTO recordings (
                id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                patient_name, audio_path, duration_seconds, file_size_bytes,
                stt_provider, ai_provider, tags, processing_status, created_at, metadata
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18
             )",
            rusqlite::params![
                recording.id.to_string(),
                recording.filename,
                recording.transcript,
                recording.soap_note,
                recording.referral,
                recording.letter,
                recording.peer_discussion,
                recording.chat,
                recording.patient_name,
                recording.audio_path.to_string_lossy().as_ref(),
                recording.duration_seconds,
                recording.file_size_bytes.map(|n| n as i64),
                recording.stt_provider,
                recording.ai_provider,
                tags_json,
                status_json,
                recording.created_at.to_rfc3339(),
                metadata_json,
            ],
        )?;
        Ok(())
    }

    /// Fetch a single recording by its UUID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if no recording with the given ID exists.
    pub fn get_by_id(conn: &Connection, id: &Uuid) -> DbResult<Recording> {
        let id_str = id.to_string();
        conn.query_row(
            "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                    patient_name, audio_path, duration_seconds, file_size_bytes,
                    stt_provider, ai_provider, tags, processing_status, created_at, metadata
             FROM recordings
             WHERE id = ?1",
            [&id_str],
            Self::row_to_recording,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                DbError::NotFound(format!("recording {id_str}"))
            }
            other => DbError::Sqlite(other),
        })
    }

    /// Return a page of lightweight summaries, newest first.
    ///
    /// Results are ordered by `created_at DESC`. Use `limit` and `offset` for
    /// pagination.
    pub fn list_all(conn: &Connection, limit: u32, offset: u32) -> DbResult<Vec<RecordingSummary>> {
        let mut stmt = conn.prepare(
            "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                    patient_name, audio_path, duration_seconds, file_size_bytes,
                    stt_provider, ai_provider, tags, processing_status, created_at, metadata
             FROM recordings
             WHERE deleted_at IS NULL
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let recordings = stmt
            .query_map(rusqlite::params![limit, offset], |row| {
                Self::row_to_recording(row)
            })?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable row"))
                    .ok()
            })
            .map(|rec| RecordingSummary::from(&rec))
            .collect();

        Ok(recordings)
    }

    /// Replace all mutable fields of an existing recording.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if the recording does not exist.
    pub fn update(conn: &Connection, recording: &Recording) -> DbResult<()> {
        let status_json = serde_json::to_string(&recording.status)
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let tags_json = serde_json::to_string(&recording.tags)
            .map_err(|e| DbError::Migration(e.to_string()))?;
        let metadata_json = recording.metadata.to_string();

        let rows = conn.execute(
            "UPDATE recordings SET
                filename = ?1,
                transcript = ?2,
                soap_note = ?3,
                referral = ?4,
                letter = ?5,
                peer_discussion = ?6,
                chat = ?7,
                patient_name = ?8,
                audio_path = ?9,
                duration_seconds = ?10,
                file_size_bytes = ?11,
                stt_provider = ?12,
                ai_provider = ?13,
                tags = ?14,
                processing_status = ?15,
                metadata = ?16
             WHERE id = ?17",
            rusqlite::params![
                recording.filename,
                recording.transcript,
                recording.soap_note,
                recording.referral,
                recording.letter,
                recording.peer_discussion,
                recording.chat,
                recording.patient_name,
                recording.audio_path.to_string_lossy().as_ref(),
                recording.duration_seconds,
                recording.file_size_bytes.map(|n| n as i64),
                recording.stt_provider,
                recording.ai_provider,
                tags_json,
                status_json,
                metadata_json,
                recording.id.to_string(),
            ],
        )?;

        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {}", recording.id)));
        }
        Ok(())
    }

    /// Delete a recording by ID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if the recording does not exist.
    pub fn delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let rows = conn.execute("DELETE FROM recordings WHERE id = ?1", [id.to_string()])?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {id}")));
        }
        Ok(())
    }

    /// Soft-delete: mark a recording as deleted without removing the row.
    ///
    /// The recording is hidden from all queries (via `deleted_at IS NULL`
    /// filtering). The frontend shows an Undo toast; if the user clicks Undo,
    /// [`restore`](Self::restore) clears the `deleted_at` field. A future purge
    /// sweeper will permanently delete soft-deleted recordings after 30 days.
    pub fn soft_delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let rows = conn.execute(
            "UPDATE recordings SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now, id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {id}")));
        }
        // Remove from FTS so search doesn't surface the soft-deleted recording.
        let _ = conn.execute(
            "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             VALUES('delete', (SELECT rowid FROM recordings WHERE id = ?1), ?1, '', '', '', '', '', '')",
            [id.to_string()],
        );
        Ok(())
    }

    /// Restore a soft-deleted recording (undo).
    ///
    /// Clears `deleted_at` so the recording reappears in queries. Also
    /// re-inserts the FTS row so search finds it again.
    pub fn restore(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let rows = conn.execute(
            "UPDATE recordings SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!(
                "recording {id} (not deleted or not found)"
            )));
        }
        // Re-insert into FTS by touching the row (the update trigger rebuilds it).
        conn.execute(
            "UPDATE recordings SET metadata = metadata WHERE id = ?1",
            [id.to_string()],
        )?;
        Ok(())
    }

    /// Delete all recordings. Returns the audio paths so callers can clean up
    /// files on disk.
    /// Permanently delete all visible (non-soft-deleted) recordings.
    ///
    /// This is a **hard DELETE** — it permanently removes recordings that have
    /// not been soft-deleted. Used by the "Delete All" button in settings,
    /// which is explicitly a destructive action with confirmation. Unlike the
    /// single-record soft-delete path, there is no undo for delete-all.
    ///
    /// Returns the audio paths so the caller can clean up files on disk.
    pub fn delete_all(conn: &Connection) -> DbResult<Vec<PathBuf>> {
        let mut stmt =
            conn.prepare("SELECT audio_path FROM recordings WHERE deleted_at IS NULL")?;
        let paths: Vec<PathBuf> = stmt
            .query_map([], |row| {
                let p: String = row.get(0)?;
                Ok(PathBuf::from(p))
            })?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable row"))
                    .ok()
            })
            .collect();

        conn.execute("DELETE FROM recordings WHERE deleted_at IS NULL", [])?;
        Ok(paths)
    }

    /// Total number of recordings in the table.
    ///
    /// Useful for pagination UI without fetching full rows.
    pub fn count(conn: &Connection) -> DbResult<u32> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM recordings WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    /// Flip any recordings stuck in `Processing` state to `Failed` with the
    /// given reason.
    ///
    /// Called at app startup so prior-session crashes or hard quits don't
    /// leave recordings permanently spinning.
    ///
    /// Returns the number of rows updated.
    pub fn fail_stuck_processing(conn: &Connection, reason: &str) -> DbResult<u32> {
        // Status is stored as serialized JSON tagged with `"status":"processing"`.
        // Match the tag as a stable substring — cheaper than deserialising every row.
        let failed = ProcessingStatus::Failed {
            error: reason.to_string(),
            retry_count: 0,
        };
        let failed_json =
            serde_json::to_string(&failed).map_err(|e| DbError::Migration(e.to_string()))?;
        let updated = conn.execute(
            "UPDATE recordings
             SET processing_status = ?1
             WHERE processing_status LIKE '%\"status\":\"processing\"%'
             AND deleted_at IS NULL",
            rusqlite::params![failed_json],
        )?;
        Ok(updated as u32)
    }

    /// Fetch multiple recordings by ID in a single query.
    ///
    /// Order is not guaranteed; sort the result on the caller side if needed.
    /// An empty `ids` slice returns an empty `Vec` without hitting the database.
    pub fn get_many(conn: &Connection, ids: &[uuid::Uuid]) -> DbResult<Vec<Recording>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat, \
                     patient_name, audio_path, duration_seconds, file_size_bytes, \
                     stt_provider, ai_provider, tags, processing_status, created_at, metadata \
             FROM recordings WHERE id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let id_strings: Vec<String> = ids.iter().map(|u| u.to_string()).collect();
        let params: Vec<&dyn rusqlite::ToSql> = id_strings
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params.as_slice(), Self::row_to_recording)?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable row"))
                    .ok()
            })
            .collect();
        Ok(rows)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Convert a SQLite row into a `Recording`.  JSON fields fall back to
    /// safe defaults on parse failure rather than propagating an error.
    pub fn row_to_recording(row: &Row) -> rusqlite::Result<Recording> {
        let id_str: String = row.get(0)?;
        let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::nil());

        let filename: String = row.get(1)?;
        let transcript: Option<String> = row.get(2)?;
        let soap_note: Option<String> = row.get(3)?;
        let referral: Option<String> = row.get(4)?;
        let letter: Option<String> = row.get(5)?;
        let peer_discussion: Option<String> = row.get(6)?;
        let chat: Option<String> = row.get(7)?;
        let patient_name: Option<String> = row.get(8)?;
        let audio_path_str: Option<String> = row.get(9)?;
        let duration_seconds: Option<f64> = row.get(10)?;
        let file_size_bytes: Option<i64> = row.get(11)?;
        let stt_provider: Option<String> = row.get(12)?;
        let ai_provider: Option<String> = row.get(13)?;

        let tags_json: Option<String> = row.get(14)?;
        let tags: Vec<String> = tags_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let status_json: Option<String> = row.get(15)?;
        let status: ProcessingStatus = status_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(ProcessingStatus::Pending);

        let created_at_str: String = row.get(16)?;
        let created_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let metadata_str: Option<String> = row.get(17)?;
        let metadata: serde_json::Value = metadata_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null);

        Ok(Recording {
            id,
            filename,
            transcript,
            soap_note,
            referral,
            letter,
            peer_discussion,
            chat,
            patient_name,
            audio_path: PathBuf::from(audio_path_str.unwrap_or_default()),
            duration_seconds,
            file_size_bytes: file_size_bytes.map(|n| n as u64),
            stt_provider,
            ai_provider,
            tags,
            status,
            created_at,
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;
    use rusqlite::Connection;

    fn migrated_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    fn new_rec() -> Recording {
        Recording::new("test.wav", PathBuf::from("/audio/test.wav"))
    }

    #[test]
    fn insert_and_retrieve() {
        let conn = migrated_conn();
        let rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        let fetched = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(fetched.id, rec.id);
        assert_eq!(fetched.filename, rec.filename);
    }

    #[test]
    fn get_nonexistent_not_found() {
        let conn = migrated_conn();
        let id = Uuid::new_v4();
        let result = RecordingsRepo::get_by_id(&conn, &id);
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[test]
    fn update_recording() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        rec.patient_name = Some("Dr. House".into());
        rec.transcript = Some("Hello world".into());
        RecordingsRepo::update(&conn, &rec).unwrap();
        let fetched = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(fetched.patient_name.as_deref(), Some("Dr. House"));
        assert_eq!(fetched.transcript.as_deref(), Some("Hello world"));
    }

    #[test]
    fn delete_recording() {
        let conn = migrated_conn();
        let rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        RecordingsRepo::delete(&conn, &rec.id).unwrap();
        assert!(matches!(
            RecordingsRepo::get_by_id(&conn, &rec.id),
            Err(DbError::NotFound(_))
        ));
    }

    #[test]
    fn list_with_pagination() {
        let conn = migrated_conn();
        for _ in 0..5 {
            RecordingsRepo::insert(&conn, &new_rec()).unwrap();
        }
        let page1 = RecordingsRepo::list_all(&conn, 3, 0).unwrap();
        let page2 = RecordingsRepo::list_all(&conn, 3, 3).unwrap();
        assert_eq!(page1.len(), 3);
        assert_eq!(page2.len(), 2);
    }

    #[test]
    fn count() {
        let conn = migrated_conn();
        assert_eq!(RecordingsRepo::count(&conn).unwrap(), 0);
        RecordingsRepo::insert(&conn, &new_rec()).unwrap();
        RecordingsRepo::insert(&conn, &new_rec()).unwrap();
        assert_eq!(RecordingsRepo::count(&conn).unwrap(), 2);
    }

    #[test]
    fn tags_round_trip() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        rec.tags = vec!["urgent".into(), "follow-up".into()];
        RecordingsRepo::insert(&conn, &rec).unwrap();
        let fetched = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(fetched.tags, vec!["urgent", "follow-up"]);
    }

    #[test]
    fn get_many_returns_matching_recordings() {
        let conn = migrated_conn();
        let r1 = {
            let r = Recording::new("first.wav", PathBuf::from("/audio/first.wav"));
            RecordingsRepo::insert(&conn, &r).unwrap();
            r
        };
        let r2 = {
            let r = Recording::new("second.wav", PathBuf::from("/audio/second.wav"));
            RecordingsRepo::insert(&conn, &r).unwrap();
            r
        };
        let _r3 = {
            let r = Recording::new("third.wav", PathBuf::from("/audio/third.wav"));
            RecordingsRepo::insert(&conn, &r).unwrap();
            r
        };

        let results = RecordingsRepo::get_many(&conn, &[r1.id, r2.id]).unwrap();
        assert_eq!(results.len(), 2);
        let ids: std::collections::HashSet<_> = results.iter().map(|r| r.id).collect();
        assert!(ids.contains(&r1.id));
        assert!(ids.contains(&r2.id));
    }

    #[test]
    fn get_many_empty_ids_returns_empty() {
        let conn = migrated_conn();
        let _r1 = {
            let r = Recording::new("first.wav", PathBuf::from("/audio/first.wav"));
            RecordingsRepo::insert(&conn, &r).unwrap();
            r
        };
        let results = RecordingsRepo::get_many(&conn, &[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn fail_stuck_processing_flips_matching_rows() {
        let conn = migrated_conn();

        // One Processing, one Pending, one already Completed.
        let mut stuck = new_rec();
        stuck.status = ProcessingStatus::Processing {
            started_at: Utc::now(),
        };
        let pending = new_rec();
        let mut done = new_rec();
        done.status = ProcessingStatus::Completed {
            completed_at: Utc::now(),
        };

        RecordingsRepo::insert(&conn, &stuck).unwrap();
        RecordingsRepo::insert(&conn, &pending).unwrap();
        RecordingsRepo::insert(&conn, &done).unwrap();

        let n = RecordingsRepo::fail_stuck_processing(&conn, "app restarted").unwrap();
        assert_eq!(n, 1);

        let after = RecordingsRepo::get_by_id(&conn, &stuck.id).unwrap();
        assert!(matches!(after.status, ProcessingStatus::Failed { .. }));

        // Others untouched.
        let p = RecordingsRepo::get_by_id(&conn, &pending.id).unwrap();
        assert!(matches!(p.status, ProcessingStatus::Pending));
        let c = RecordingsRepo::get_by_id(&conn, &done.id).unwrap();
        assert!(matches!(c.status, ProcessingStatus::Completed { .. }));
    }
}
