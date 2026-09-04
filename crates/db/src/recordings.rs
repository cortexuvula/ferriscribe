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

/// Column-scoped update payload for [`RecordingsRepo::persist_producer_update`].
/// `None` fields are left untouched; the metadata patch is merged into the
/// row's CURRENT metadata at persist time (see the method docs).
#[derive(Default)]
pub struct ProducerPersist {
    pub transcript: Option<String>,
    pub soap_note: Option<String>,
    pub referral: Option<String>,
    pub letter: Option<String>,
    pub peer_discussion: Option<String>,
    pub stt_provider: Option<String>,
    /// Pre-serialized `ProcessingStatus` JSON.
    pub processing_status: Option<String>,
    pub metadata_patch: Vec<(String, serde_json::Value)>,
}

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
                stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18,
                ?19
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
                recording.updated_at.unwrap_or_else(Utc::now).to_rfc3339(),
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
                    stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                    updated_at
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

    /// Fetch a single recording by its UUID, rejecting soft-deleted rows.
    ///
    /// Companion to [`Self::get_by_id`] for producer paths (generation,
    /// transcription) that must not spend minutes of LLM work on a recording
    /// the user has since moved to the trash: the persist at the end filters
    /// `deleted_at IS NULL`, so a deleted-mid-generation run would otherwise
    /// complete the whole completion and then fail with a bare `NotFound`.
    /// Restore/sync flows that legitimately operate on trashed rows keep
    /// using [`Self::get_by_id`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] — with an "is deleted" message when the
    /// row exists but is trashed — if no active recording with the given ID
    /// exists.
    pub fn get_by_id_active(conn: &Connection, id: &Uuid) -> DbResult<Recording> {
        let id_str = id.to_string();
        match conn.query_row(
            "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                    patient_name, audio_path, duration_seconds, file_size_bytes,
                    stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                    updated_at
             FROM recordings
             WHERE id = ?1 AND deleted_at IS NULL",
            [&id_str],
            Self::row_to_recording,
        ) {
            Ok(recording) => Ok(recording),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Distinguish "never existed" from "soft-deleted" so the
                // producer error is actionable. The existence probe is
                // error-path-only — one extra query on failure.
                let exists: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM recordings WHERE id = ?1)",
                        [&id_str],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if exists {
                    Err(DbError::NotFound(format!("recording {id_str} is deleted")))
                } else {
                    Err(DbError::NotFound(format!("recording {id_str}")))
                }
            }
            Err(other) => Err(DbError::Sqlite(other)),
        }
    }

    /// Return a page of lightweight summaries, newest first.
    ///
    /// Results are ordered by `created_at DESC`. Use `limit` and `offset` for
    /// pagination.
    pub fn list_all(conn: &Connection, limit: u32, offset: u32) -> DbResult<Vec<RecordingSummary>> {
        let mut stmt = conn.prepare(
            "SELECT id, filename, transcript, soap_note, referral, letter, peer_discussion, chat,
                    patient_name, audio_path, duration_seconds, file_size_bytes,
                    stt_provider, ai_provider, tags, processing_status, created_at, metadata,
                    updated_at
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

        let now_rfc3339 = Utc::now().to_rfc3339();

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
                metadata = ?16,
                updated_at = ?17
             WHERE id = ?18 AND deleted_at IS NULL",
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
                now_rfc3339,
                recording.id.to_string(),
            ],
        )?;

        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {}", recording.id)));
        }
        Ok(())
    }

    // Guard note: the WHERE clause above must keep `AND deleted_at IS NULL`.
    // Soft-deleted rows are de-indexed from the external-content FTS table;
    // an UPDATE firing the FTS update trigger on such a row fails with
    // SQLITE_CORRUPT. With the guard, updates to trashed rows are a clean
    // no-op → `NotFound` (callers surface "recording deleted").

    /// Update ONLY the audio-location columns (`audio_path`,
    /// `file_size_bytes`) of an existing recording, deliberately leaving
    /// `updated_at` untouched.
    ///
    /// Audio arrival is not a content change: the syncable fields
    /// (transcript, SOAP, …) didn't change, and the wire builder stamps
    /// every syncable field with max(revision, row). Bumping the row here
    /// would inflate those stamps with the audio arrival time and let a
    /// later pull overwrite a concurrent field edit on another machine
    /// with the older stored value — silent data loss. Callers touching
    /// actual content must use [`RecordingsRepo::update`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if the recording does not exist.
    pub fn update_audio_location(
        conn: &Connection,
        id: &Uuid,
        audio_path: &std::path::Path,
        file_size_bytes: Option<u64>,
    ) -> DbResult<()> {
        let rows = conn.execute(
            "UPDATE recordings SET
                audio_path = ?1,
                file_size_bytes = ?2
             WHERE id = ?3 AND deleted_at IS NULL",
            rusqlite::params![
                audio_path.to_string_lossy().as_ref(),
                file_size_bytes.map(|n| n as i64),
                id.to_string(),
            ],
        )?;

        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {id}")));
        }
        Ok(())
    }

    /// Targeted persist for long-running producers (transcription, SOAP
    /// generation). These paths hold a recording snapshot across
    /// minutes-long operations; a whole-row [`RecordingsRepo::update`] on
    /// the stale snapshot would silently revert any column another writer
    /// changed in the window (the editor's field-level autosave, a
    /// concurrent document generator). This writes ONLY the carried
    /// columns, and merges the metadata PATCH into the CURRENT row's
    /// metadata at persist time — never a wholesale metadata write.
    ///
    /// `updated_at` still bumps: this is a real content change, and the
    /// sync wire builder's max(revision, row) stamp relies on it.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if the recording does not exist.
    pub fn persist_producer_update(
        conn: &Connection,
        id: &Uuid,
        update: &ProducerPersist,
    ) -> DbResult<()> {
        let tx = conn.unchecked_transaction()?;

        // Persist-time metadata merge: read the row's CURRENT metadata
        // (not the producer's stale snapshot) and apply the patch. When
        // both the current value and the patch value are objects, merge
        // one level (so a `generation_stats` patch adds its doc-type key
        // without dropping sibling stats).
        let mut sets: Vec<String> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut next_idx = 1usize;

        if !update.metadata_patch.is_empty() {
            let current: Option<String> = tx.query_row(
                "SELECT metadata FROM recordings WHERE id = ?1 AND deleted_at IS NULL",
                [&id.to_string()],
                |row| row.get(0),
            )?;
            let mut metadata: serde_json::Value = current
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            if !metadata.is_object() {
                metadata = serde_json::json!({});
            }
            let obj = metadata.as_object_mut().expect("just made an object");
            for (key, value) in &update.metadata_patch {
                match (obj.get_mut(key), value) {
                    (
                        Some(serde_json::Value::Object(existing)),
                        serde_json::Value::Object(incoming),
                    ) => {
                        for (k, v) in incoming {
                            existing.insert(k.clone(), v.clone());
                        }
                    }
                    _ => {
                        obj.insert(key.clone(), value.clone());
                    }
                }
            }
            sets.push(format!("metadata = ?{next_idx}"));
            params.push(Box::new(metadata.to_string()));
            next_idx += 1;
        }

        if let Some(transcript) = &update.transcript {
            sets.push(format!("transcript = ?{next_idx}"));
            params.push(Box::new(transcript.clone()));
            next_idx += 1;
        }
        if let Some(soap_note) = &update.soap_note {
            sets.push(format!("soap_note = ?{next_idx}"));
            params.push(Box::new(soap_note.clone()));
            next_idx += 1;
        }
        if let Some(referral) = &update.referral {
            sets.push(format!("referral = ?{next_idx}"));
            params.push(Box::new(referral.clone()));
            next_idx += 1;
        }
        if let Some(letter) = &update.letter {
            sets.push(format!("letter = ?{next_idx}"));
            params.push(Box::new(letter.clone()));
            next_idx += 1;
        }
        if let Some(peer_discussion) = &update.peer_discussion {
            sets.push(format!("peer_discussion = ?{next_idx}"));
            params.push(Box::new(peer_discussion.clone()));
            next_idx += 1;
        }
        if let Some(stt_provider) = &update.stt_provider {
            sets.push(format!("stt_provider = ?{next_idx}"));
            params.push(Box::new(stt_provider.clone()));
            next_idx += 1;
        }
        if let Some(processing_status) = &update.processing_status {
            sets.push(format!("processing_status = ?{next_idx}"));
            params.push(Box::new(processing_status.clone()));
            next_idx += 1;
        }

        if sets.is_empty() && update.metadata_patch.is_empty() {
            return Ok(());
        }

        // Content change → row stamp moves (LWW rider dependency).
        let now_rfc3339 = Utc::now().to_rfc3339();
        sets.push(format!("updated_at = ?{next_idx}"));
        params.push(Box::new(now_rfc3339));
        next_idx += 1;

        params.push(Box::new(id.to_string()));
        let sql = format!(
            "UPDATE recordings SET {} WHERE id = ?{next_idx} AND deleted_at IS NULL",
            sets.join(", ")
        );

        let rows = tx.execute(
            sql.as_str(),
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        )?;
        tx.commit()?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {id}")));
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
        let now = chrono::Utc::now().to_rfc3339();
        // Single transaction: the row UPDATE and the FTS 'delete' must land
        // together. A crash between them leaves soft-deleted PHI still
        // searchable (disclosure), and a partial retry then fires the
        // trigger 'delete' against mismatched index state (SQLITE_CORRUPT).
        // `unchecked_transaction` because repo methods receive `&Connection`
        // (the crate's established pattern — see purge_soft_deleted_impl).
        let tx = conn.unchecked_transaction()?;
        let rows = tx.execute(
            "UPDATE recordings SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now, id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("recording {id}")));
        }
        // Remove from FTS so search doesn't surface the soft-deleted recording.
        // `recordings_fts` is an external-content FTS5 table: the 'delete'
        // command must be supplied the *currently indexed* column values.
        // Placeholder values ('') leave the index internally inconsistent,
        // and the next 'delete' for this rowid (e.g. the trigger fired by
        // `restore`'s UPDATE) then fails with SQLITE_CORRUPT.
        //
        // The error is propagated, not swallowed: warn-and-continue would
        // commit the row UPDATE with the row still indexed — the exact
        // inconsistent state the transaction exists to prevent. Failing
        // rolls both statements back, leaving the recording visible and
        // indexed, ready for a retry.
        tx.execute(
            "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1",
            [id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Restore a soft-deleted recording (undo).
    ///
    /// Clears `deleted_at` so the recording reappears in queries. Also
    /// re-inserts the FTS row so search finds it again.
    ///
    /// Before clearing `deleted_at`, the metadata is stamped with
    /// `retention_exempt: true` — a recording the user explicitly pulled
    /// back out of the trash must never be re-trashed by a later
    /// [`retention_soft_delete_older_than`](Self::retention_soft_delete_older_than)
    /// sweep.
    pub fn restore(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        // Single transaction for the read + FTS insert + UPDATE: the FTS
        // re-insert and the row UPDATE must land together, or a crash in
        // between re-indexes a trashed row / untrashes an unindexed one and
        // the next trigger 'delete' corrupts the index (SQLITE_CORRUPT).
        let tx = conn.unchecked_transaction()?;
        // Read the current row (metadata + trash state) up front. A missing
        // row or one that isn't soft-deleted gets the same NotFound the
        // UPDATE-based check below has always produced — checked early so
        // no FTS mutation happens for non-restorable rows.
        let (metadata, deleted_at): (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT metadata, deleted_at FROM recordings WHERE id = ?1",
                [&id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("recording {id} (not deleted or not found)"))
                }
                other => DbError::Sqlite(other),
            })?;
        if deleted_at.is_none() {
            return Err(DbError::NotFound(format!(
                "recording {id} (not deleted or not found)"
            )));
        }
        // Stamp the exemption before clearing deleted_at. The metadata
        // column is JSON (`TEXT DEFAULT 'null'`); tolerate NULL, non-JSON,
        // and non-object values by falling back to `{}`.
        let mut meta: serde_json::Value = metadata
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("retention_exempt".to_string(), serde_json::json!(true));
        }
        let metadata_json = meta.to_string();
        // Re-add the FTS row BEFORE the UPDATE. `soft_delete` removed it,
        // and the `recordings_fts_update` trigger fires a 'delete' for the
        // old values on every UPDATE — for this external-content FTS table
        // that 'delete' must match the indexed state or the index corrupts
        // (SQLITE_CORRUPT).
        //
        // The error is propagated (`?`), not swallowed: warn-and-continue
        // would let the UPDATE below fire its trigger 'delete' against
        // absent index state — corrupting the index and wedging every
        // later FTS operation. Failing the restore leaves the row
        // consistently trashed + de-indexed, ready for a retry.
        tx.execute(
            "INSERT INTO recordings_fts(rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1",
            [id.to_string()],
        )?;
        let rows = tx.execute(
            "UPDATE recordings SET deleted_at = NULL, updated_at = ?1, metadata = ?2 WHERE id = ?3 AND deleted_at IS NOT NULL",
            rusqlite::params![now, metadata_json, id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!(
                "recording {id} (not deleted or not found)"
            )));
        }
        tx.commit()?;
        Ok(())
    }

    /// Retention sweep: soft-delete every visible recording older than the
    /// cutoff whose metadata does not carry `retention_exempt`. Returns the
    /// ids trashed (for count-only logging — never content). Idempotent:
    /// rows already in trash are never touched.
    pub fn retention_soft_delete_older_than(
        conn: &Connection,
        days: u32,
        now: DateTime<Utc>,
    ) -> DbResult<Vec<Uuid>> {
        let cutoff = (now - chrono::TimeDelta::days(days as i64)).to_rfc3339();
        // `datetime(...) < datetime(?)` matches the tombstone sweeper's
        // convention (src-tauri state.rs) and correctly compares the
        // RFC3339-with-offset strings this table stores.
        let mut stmt = conn.prepare(
            "SELECT id, metadata FROM recordings
              WHERE deleted_at IS NULL AND datetime(created_at) < datetime(?1)",
        )?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(rusqlite::params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;

        let mut trashed = Vec::new();
        for (id_str, metadata) in rows {
            let Ok(id) = Uuid::parse_str(&id_str) else {
                continue;
            };
            let exempt = metadata
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|m| m.get("retention_exempt").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            if exempt {
                continue;
            }
            Self::soft_delete(conn, &id)?;
            trashed.push(id);
        }
        Ok(trashed)
    }

    /// List soft-deleted recordings older than the cutoff (days), for callers
    /// that clean up external resources (RAG vectors, audio files) before
    /// purging rows. Returns (id, audio_path) pairs.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] if the query fails. Rows with an
    /// unparseable id are skipped (logged) rather than aborting the sweep.
    pub fn list_soft_deleted_older_than(
        conn: &Connection,
        days: u32,
        now: DateTime<Utc>,
    ) -> DbResult<Vec<(Uuid, String)>> {
        let cutoff = (now - chrono::TimeDelta::days(days as i64)).to_rfc3339();
        let mut stmt = conn.prepare(
            "SELECT id, audio_path FROM recordings
              WHERE deleted_at IS NOT NULL AND datetime(deleted_at) < datetime(?1)",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .filter_map(|r| {
                r.map_err(
                    |e| tracing::warn!(error = %e, "dropping unreadable row in tombstone listing"),
                )
                .ok()
            })
            .filter_map(|(id_str, path)| match Uuid::parse_str(&id_str) {
                Ok(id) => Some((id, path.unwrap_or_default())),
                Err(e) => {
                    tracing::warn!(error = %e, "unparseable recording id in tombstone listing");
                    None
                }
            })
            .collect();
        Ok(rows)
    }

    /// Hard-delete soft-deleted rows by id, keeping the FTS index consistent:
    /// re-inserts each row into the external-content index immediately before
    /// the DELETE so the delete-trigger's 'delete' command matches indexed
    /// state. Transactional — all rows or none. Skips ids that are not
    /// currently soft-deleted (safety). Returns the ids actually purged.
    ///
    /// `soft_delete` removed these rowids from `recordings_fts` when they
    /// were trashed; without the re-insert, the `recordings_fts_delete`
    /// trigger issues a 'delete' for a rowid the index no longer holds,
    /// which SQLite reports as `SQLITE_CORRUPT` ("database disk image is
    /// malformed") — the raw `DELETE` this replaces never succeeded.
    ///
    /// The server-side tombstone sweeper should call
    /// [`RecordingsRepo::purge_soft_deleted_with_ledger`] instead, which
    /// additionally records each purged id in the `purged_recordings`
    /// resurrection-blocking ledger inside the same transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] on any failure; the transaction is rolled
    /// back and no rows are removed.
    pub fn purge_soft_deleted(conn: &Connection, ids: &[Uuid]) -> DbResult<Vec<Uuid>> {
        Self::purge_soft_deleted_impl(conn, ids, false)
    }

    /// [`RecordingsRepo::purge_soft_deleted`], plus a write to the
    /// `purged_recordings` ledger for every id actually purged — in the SAME
    /// transaction as the row deletion, so a durable deletion can never land
    /// without its resurrection block (or vice versa).
    ///
    /// The ledger is what lets `ContentSyncRepo::merge_incoming` refuse a
    /// stale live copy of the recording pushed later by a machine that
    /// missed the practice-wide deletion: same-UUID + ledger hit is always a
    /// stale copy, since genuinely re-created content gets a new UUID.
    ///
    /// **HIPAA note:** the ledger stores the recording id and a purge
    /// timestamp only — never filenames, transcripts, or any other content.
    ///
    /// Only ids whose rows were actually deleted are ledgered; a visible id
    /// passed by mistake is neither purged nor ledgered (a spurious ledger
    /// entry would over-block a later legitimate sync insert of that id).
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] on any failure; the transaction is rolled
    /// back, no rows are removed, and no ledger entries are written.
    pub fn purge_soft_deleted_with_ledger(conn: &Connection, ids: &[Uuid]) -> DbResult<Vec<Uuid>> {
        Self::purge_soft_deleted_impl(conn, ids, true)
    }

    /// Shared body of the two purge entry points. See their docs; the flag
    /// selects whether purged ids are also written to the
    /// `purged_recordings` ledger.
    fn purge_soft_deleted_impl(
        conn: &Connection,
        ids: &[Uuid],
        write_ledger: bool,
    ) -> DbResult<Vec<Uuid>> {
        // `unchecked_transaction` because repo methods receive `&Connection`
        // (the crate's established pattern — see migrations/mod.rs). If any
        // statement fails, the transaction's Drop rolls everything back.
        let tx = conn.unchecked_transaction()?;
        let purged_at = Utc::now().to_rfc3339();
        let mut purged = Vec::new();
        for id in ids {
            let id_str = id.to_string();
            // Re-index the row with its currently stored values (the exact
            // column list `restore` re-inserts / `soft_delete` deletes), but
            // only while it is still soft-deleted. The WHERE clause doubles
            // as the visible-row guard: for a visible id the SELECT yields
            // no rows, so the FTS index is never touched.
            tx.execute(
                "INSERT INTO recordings_fts(rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
                 SELECT rowid, id, filename, transcript, soap_note, referral, letter, patient_name
                 FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL",
                [&id_str],
            )?;
            // The delete trigger now finds matching indexed values, so the
            // hard DELETE de-indexes cleanly instead of corrupting FTS.
            let rows = tx.execute(
                "DELETE FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL",
                [&id_str],
            )?;
            if rows > 0 {
                if write_ledger {
                    // Upsert keeps re-purges (and any replay of this batch)
                    // idempotent while refreshing the timestamp. Same
                    // transaction as the DELETE above — atomic by design.
                    tx.execute(
                        "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)
                         ON CONFLICT(id) DO UPDATE SET purged_at = excluded.purged_at",
                        rusqlite::params![id_str, purged_at],
                    )?;
                }
                purged.push(*id);
            }
        }
        tx.commit()?;
        Ok(purged)
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

    /// Mark a recording's background encryption as complete (clears the
    /// `encryption_pending` flag set by `stop_recording`).
    ///
    /// Called from the `spawn_blocking` task that encrypts the WAV file, on
    /// both success and failure — the flag is "encryption attempt finished",
    /// not "encryption succeeded". On failure the recording is left as
    /// plaintext at rest, which the reader (`open_recording_wav`) handles
    /// transparently.
    pub fn set_encryption_done(conn: &Connection, id: &Uuid) -> DbResult<()> {
        // FTS guard: `soft_delete` de-indexes trashed rows from the
        // external-content `recordings_fts` table, but the UPDATE below
        // fires `recordings_fts_update` even though `encryption_pending`
        // isn't an indexed column — its 'delete' command would run against
        // absent index state and fail with SQLITE_CORRUPT. Re-index the row
        // first with the same guarded INSERT..SELECT `purge_soft_deleted`
        // uses; for a visible row the `deleted_at IS NOT NULL` filter
        // matches nothing and the index is untouched.
        //
        // Trashed recordings deliberately still reach this point (their
        // audio must be encrypted at rest — better posture, and purge
        // removes the file later). After the UPDATE a trashed row is
        // indexed again but still hidden from queries (they filter
        // `deleted_at`), and the purge path stays safe: its DELETE-trigger
        // 'delete' then matches indexed state.
        //
        // Both statements in one transaction so the re-index and the flag
        // UPDATE land together (same crash-consistency rationale as
        // `soft_delete`/`restore`).
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO recordings_fts(rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL",
            [&id.to_string()],
        )?;
        tx.execute(
            "UPDATE recordings SET encryption_pending = 0 WHERE id = ?1",
            [&id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Return the `(id, audio_path)` of every recording still flagged
    /// `encryption_pending = 1`.
    ///
    /// Used by the startup sweep to encrypt WAVs left plaintext by a crash
    /// or hard-quit mid-encryption. Rows with a missing/unparseable id or
    /// path are dropped (they can't be encrypted by name).
    pub fn list_encryption_pending(conn: &Connection) -> DbResult<Vec<(Uuid, PathBuf)>> {
        let mut stmt =
            conn.prepare("SELECT id, audio_path FROM recordings WHERE encryption_pending = 1")?;
        let rows = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let path: String = row.get(1)?;
                let id = Uuid::parse_str(&id_str).map_err(|e| {
                    tracing::error!(id_str = %id_str, error = %e, "corrupt recording id in encryption_pending");
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok((id, PathBuf::from(path)))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
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
                     stt_provider, ai_provider, tags, processing_status, created_at, metadata, \
                     updated_at \
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
        let id = Uuid::parse_str(&id_str).map_err(|e| {
            tracing::error!(id_str = %id_str, error = %e, "corrupt recording id in DB");
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;

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
        let created_at: DateTime<Utc> =
            crate::parse_db_timestamp(16, &created_at_str, "recordings.created_at")?;

        let metadata_str: Option<String> = row.get(17)?;
        let metadata: serde_json::Value = metadata_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null);

        // `updated_at` is read by column name (robust to position) and falls
        // back to `None` if the column is missing/NULL.
        let updated_at: Option<DateTime<Utc>> = row
            .get::<_, Option<String>>("updated_at")
            .ok()
            .flatten()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

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
            updated_at,
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

    // Regression (2026-09-04 SOAP pipeline review): producer paths must
    // fail fast on soft-deleted rows instead of spending the whole LLM call
    // and failing the persist afterwards.
    #[test]
    fn get_by_id_active_rejects_soft_deleted_with_distinct_message() {
        let conn = migrated_conn();
        let rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        RecordingsRepo::soft_delete(&conn, &rec.id).unwrap();

        // The plain lookup still sees the trashed row (restore flows).
        assert!(RecordingsRepo::get_by_id(&conn, &rec.id).is_ok());
        // The active-only lookup rejects it, saying why.
        let err = RecordingsRepo::get_by_id_active(&conn, &rec.id)
            .expect_err("soft-deleted row must be rejected");
        let msg = format!("{err}");
        assert!(matches!(err, DbError::NotFound(_)));
        assert!(
            msg.contains("deleted"),
            "expected 'is deleted' message: {msg}"
        );
    }

    #[test]
    fn get_by_id_active_returns_live_rows_and_names_missing_ones() {
        let conn = migrated_conn();
        let rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        let fetched = RecordingsRepo::get_by_id_active(&conn, &rec.id).unwrap();
        assert_eq!(fetched.id, rec.id);

        let err = RecordingsRepo::get_by_id_active(&conn, &Uuid::new_v4())
            .expect_err("missing row must be NotFound");
        let msg = format!("{err}");
        assert!(matches!(err, DbError::NotFound(_)));
        assert!(!msg.contains("deleted"), "missing is not deleted: {msg}");
    }

    // Regression (2026-09-02 bug review): audio-location writes must NOT
    // bump `updated_at`. The wire builder stamps every syncable field with
    // max(revision, row); an audio arrival bumping the row inflated those
    // stamps and let a later pull overwrite a concurrent field edit on
    // another machine with the older stored value — silent data loss.
    #[test]
    fn update_audio_location_preserves_updated_at_and_content() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        rec.transcript = Some("v1".into());
        RecordingsRepo::insert(&conn, &rec).unwrap();
        RecordingsRepo::update(&conn, &rec).unwrap(); // stamps updated_at = now
        let before = RecordingsRepo::get_by_id(&conn, &rec.id)
            .unwrap()
            .updated_at
            .expect("row stamp set");

        std::thread::sleep(std::time::Duration::from_millis(10));
        RecordingsRepo::update_audio_location(
            &conn,
            &rec.id,
            std::path::Path::new("/audio/remote.enc"),
            Some(42),
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(after.updated_at, Some(before), "row stamp must not move");
        assert_eq!(after.audio_path, PathBuf::from("/audio/remote.enc"));
        assert_eq!(after.file_size_bytes, Some(42));
        assert_eq!(after.transcript.as_deref(), Some("v1"), "content untouched");
    }

    #[test]
    fn update_audio_location_missing_row_is_not_found() {
        let conn = migrated_conn();
        let result = RecordingsRepo::update_audio_location(
            &conn,
            &Uuid::new_v4(),
            std::path::Path::new("/x"),
            None,
        );
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    // Regression (2026-09-02 bug review): transcription/SOAP hold a
    // minutes-stale snapshot; a whole-row update reverted concurrent
    // column edits. The producer persist must touch ONLY its columns.
    #[test]
    fn persist_producer_update_does_not_revert_concurrent_column_edits() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        rec.transcript = Some("stale".into());
        rec.referral = Some("old referral".into());
        RecordingsRepo::insert(&conn, &rec).unwrap();

        // While the "producer" was running, the editor saved a new referral.
        let mut edited = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        edited.referral = Some("edited referral".into());
        RecordingsRepo::update(&conn, &edited).unwrap();

        // The producer finishes and persists only its own columns.
        RecordingsRepo::persist_producer_update(
            &conn,
            &rec.id,
            &ProducerPersist {
                transcript: Some("fresh transcript".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(after.transcript.as_deref(), Some("fresh transcript"));
        assert_eq!(
            after.referral.as_deref(),
            Some("edited referral"),
            "concurrent column edit must survive the producer persist"
        );
    }

    #[test]
    fn persist_producer_update_writes_doc_columns_without_touching_siblings() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        rec.soap_note = Some("S: stable SOAP".into());
        RecordingsRepo::insert(&conn, &rec).unwrap();

        // Each of the three document columns persists, and none disturbs
        // the SOAP column or the other doc columns (each writes only its own).
        let columns = [
            (
                "referral",
                ProducerPersist {
                    referral: Some("referral text".into()),
                    ..Default::default()
                },
            ),
            (
                "letter",
                ProducerPersist {
                    letter: Some("letter text".into()),
                    ..Default::default()
                },
            ),
            (
                "peer",
                ProducerPersist {
                    peer_discussion: Some("peer discussion text".into()),
                    ..Default::default()
                },
            ),
        ];
        for (_label, update) in &columns {
            RecordingsRepo::persist_producer_update(&conn, &rec.id, update).unwrap();
            let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
            assert_eq!(after.soap_note.as_deref(), Some("S: stable SOAP"));
        }
        // After all three producers ran, each column carries its own text —
        // no whole-row revert, no sibling disturbance.
        let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(after.soap_note.as_deref(), Some("S: stable SOAP"));
        assert_eq!(after.referral.as_deref(), Some("referral text"));
        assert_eq!(after.letter.as_deref(), Some("letter text"));
        assert_eq!(
            after.peer_discussion.as_deref(),
            Some("peer discussion text")
        );
    }

    #[test]
    fn persist_producer_update_merges_metadata_patch_into_current_row() {
        let conn = migrated_conn();
        let mut rec = new_rec();
        rec.metadata = serde_json::json!({
            "context": "visit notes",
            "generation_stats": { "referral": { "model": "m1" } }
        });
        RecordingsRepo::insert(&conn, &rec).unwrap();

        RecordingsRepo::persist_producer_update(
            &conn,
            &rec.id,
            &ProducerPersist {
                soap_note: Some("S: cough".into()),
                metadata_patch: vec![
                    ("icd_codes".into(), serde_json::json!([{"code": "786.2"}])),
                    (
                        "generation_stats".into(),
                        serde_json::json!({ "soap": { "model": "m2" } }),
                    ),
                ],
                ..Default::default()
            },
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert_eq!(after.soap_note.as_deref(), Some("S: cough"));
        let meta = after.metadata;
        assert_eq!(meta["context"], "visit notes", "unrelated keys preserved");
        assert_eq!(meta["icd_codes"][0]["code"], "786.2");
        assert_eq!(
            meta["generation_stats"]["referral"]["model"], "m1",
            "sibling stats preserved (one-level merge)"
        );
        assert_eq!(meta["generation_stats"]["soap"]["model"], "m2");
    }

    #[test]
    fn persist_producer_update_bumps_updated_at() {
        let conn = migrated_conn();
        let rec = new_rec();
        RecordingsRepo::insert(&conn, &rec).unwrap();
        let before = RecordingsRepo::get_by_id(&conn, &rec.id)
            .unwrap()
            .updated_at
            .expect("stamp");

        std::thread::sleep(std::time::Duration::from_millis(10));
        RecordingsRepo::persist_producer_update(
            &conn,
            &rec.id,
            &ProducerPersist {
                transcript: Some("t".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let after = RecordingsRepo::get_by_id(&conn, &rec.id).unwrap();
        assert!(
            after.updated_at.unwrap() > before,
            "content change bumps the row stamp"
        );
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
