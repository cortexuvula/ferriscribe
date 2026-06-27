//! FTS5-backed full-text search over recordings.

use rusqlite::Connection;
use uuid::Uuid;

use medical_core::types::recording::Recording;

use crate::{DbResult, recordings::RecordingsRepo};

/// Repository for full-text search over recordings via FTS5.
///
/// Uses the `recordings_fts` virtual table (kept in sync by SQLite triggers)
/// to perform BM25-ranked queries across transcript, SOAP note, referral,
/// letter, and patient name fields.
pub struct SearchRepo;

impl SearchRepo {
    /// Search recordings using FTS5 MATCH.
    ///
    /// Returns the UUIDs of matching rows ordered by rank (best match first).
    /// An empty or whitespace-only query returns an empty vector.
    pub fn search(conn: &Connection, query: &str, limit: u32) -> DbResult<Vec<Uuid>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn.prepare(
            "SELECT id FROM recordings_fts
             WHERE recordings_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let ids: Vec<Uuid> = stmt
            .query_map(rusqlite::params![trimmed, limit], |row| {
                let id_str: String = row.get(0)?;
                Ok(id_str)
            })?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable row"))
                    .ok()
            })
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect();

        Ok(ids)
    }

    /// Like `search`, but resolves each matching UUID to a full `Recording`.
    ///
    /// Convenience wrapper that combines [`SearchRepo::search`] with
    /// [`RecordingsRepo::get_many`](crate::recordings::RecordingsRepo::get_many).
    pub fn search_recordings(
        conn: &Connection,
        query: &str,
        limit: u32,
    ) -> DbResult<Vec<Recording>> {
        let ids = Self::search(conn, query, limit)?;
        RecordingsRepo::get_many(conn, &ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;
    use crate::recordings::RecordingsRepo;
    use rusqlite::Connection;
    use std::path::PathBuf;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    fn new_rec_with(
        filename: &str,
        transcript: Option<&str>,
        patient_name: Option<&str>,
    ) -> Recording {
        let mut rec = Recording::new(filename, PathBuf::from("/audio/test.wav"));
        rec.transcript = transcript.map(String::from);
        rec.patient_name = patient_name.map(String::from);
        rec
    }

    #[test]
    fn empty_query_empty() {
        let conn = migrated();
        let results = SearchRepo::search(&conn, "", 10).unwrap();
        assert!(results.is_empty());
        let results = SearchRepo::search(&conn, "   ", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn finds_by_transcript() {
        let conn = migrated();
        let rec = new_rec_with("visit.wav", Some("patient has hypertension"), None);
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "hypertension", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], id);
    }

    #[test]
    fn finds_by_soap_note() {
        let conn = migrated();
        let mut rec = Recording::new("soap.wav", PathBuf::from("/audio/soap.wav"));
        rec.soap_note = Some("Assessment: diabetes mellitus type 2".into());
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "diabetes", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], id);
    }

    #[test]
    fn finds_by_patient_name() {
        let conn = migrated();
        let rec = new_rec_with("name.wav", None, Some("Jane Doe"));
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "Jane", 10).unwrap();
        assert!(results.contains(&id));
    }

    #[test]
    fn respects_limit() {
        let conn = migrated();
        for i in 0..5 {
            let rec = new_rec_with(
                &format!("rec{i}.wav"),
                Some("common keyword search term"),
                None,
            );
            RecordingsRepo::insert(&conn, &rec).unwrap();
        }

        let results = SearchRepo::search(&conn, "keyword", 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn fts_updates_on_recording_update() {
        let conn = migrated();
        let mut rec = Recording::new("upd.wav", PathBuf::from("/audio/upd.wav"));
        rec.transcript = Some("original content".into());
        RecordingsRepo::insert(&conn, &rec).unwrap();

        // Should find by old term
        let old = SearchRepo::search(&conn, "original", 10).unwrap();
        assert_eq!(old.len(), 1);

        // Update transcript
        rec.transcript = Some("updated content entirely different".into());
        RecordingsRepo::update(&conn, &rec).unwrap();

        // Should find by new term
        let new_results = SearchRepo::search(&conn, "entirely", 10).unwrap();
        assert_eq!(new_results.len(), 1);

        // Old term should no longer match
        let old_after = SearchRepo::search(&conn, "original", 10).unwrap();
        assert!(old_after.is_empty());
    }
}
