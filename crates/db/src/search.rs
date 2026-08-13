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
    ///
    /// # Query escaping
    ///
    /// The user's input is passed to FTS5 as **query syntax**, not a literal
    /// string — so raw input containing FTS metacharacters breaks the parse.
    /// Worst case in practice: hyphens (filenames like
    /// `Recording_2026-08-13_13-39-22.wav`, dates like `2026-08-13`, clinical
    /// terms like `covid-19`) make FTS5 treat the following token as a column
    /// filter and fail with `no such column`, which surfaced as a silent
    /// empty result set. To make input literal, each whitespace-separated term
    /// is wrapped as an FTS5 **phrase** (`"term"`); multiple phrases form an
    /// implicit AND, preserving multi-word semantics. FTS5 phrases cannot
    /// contain double quotes, so stray quotes are stripped.
    pub fn search(conn: &Connection, query: &str, limit: u32) -> DbResult<Vec<Uuid>> {
        let phrases: Vec<String> = query
            .split_whitespace()
            .filter_map(|term| {
                let cleaned = term.replace('"', "");
                if cleaned.is_empty() {
                    None
                } else {
                    Some(format!("\"{cleaned}\""))
                }
            })
            .collect();
        if phrases.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = phrases.join(" ");

        let mut stmt = conn.prepare(
            "SELECT id FROM recordings_fts
             WHERE recordings_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let ids: Vec<Uuid> = stmt
            .query_map(rusqlite::params![fts_query, limit], |row| {
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

    #[test]
    fn finds_by_filename_with_hyphens() {
        // Regression: raw queries were passed to FTS5 MATCH as query syntax,
        // so hyphens in a filename made FTS5 fail with "no such column" and
        // the search silently returned nothing. The user-visible symptom was
        // being unable to find synced recordings by filename.
        let conn = migrated();
        let rec = new_rec_with(
            "Recording_2026-08-13_13-39-22.wav",
            None,
            None, // no patient name — filename is the only identifying text
        );
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        // Full filename search must find it (previously: Err "no such column: 08").
        let results = SearchRepo::search(&conn, "Recording_2026-08-13_13-39-22.wav", 10).unwrap();
        assert_eq!(results, vec![id]);

        // A distinctive fragment also matches the filename's tokens.
        let frag = SearchRepo::search(&conn, "13-39-22", 10).unwrap();
        assert_eq!(frag, vec![id]);
    }

    #[test]
    fn hyphenated_clinical_term_does_not_error() {
        // Terms like covid-19 or h-pylori previously broke the FTS5 parse.
        let conn = migrated();
        let rec = new_rec_with(
            "visit.wav",
            Some("patient tested positive for covid-19"),
            None,
        );
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "covid-19", 10).unwrap();
        assert_eq!(results, vec![id]);
    }

    #[test]
    fn date_query_does_not_error() {
        let conn = migrated();
        let rec = new_rec_with("note.wav", Some("follow-up on 2026-08-13 discussed"), None);
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "2026-08-13", 10).unwrap();
        assert_eq!(results, vec![id]);
    }

    #[test]
    fn multi_word_terms_are_implicit_and() {
        let conn = migrated();
        let matching = new_rec_with("a.wav", Some("hypertension noted, diabetes reviewed"), None);
        let other = new_rec_with("b.wav", Some("hypertension only"), None);
        RecordingsRepo::insert(&conn, &matching).unwrap();
        RecordingsRepo::insert(&conn, &other).unwrap();

        let results = SearchRepo::search(&conn, "hypertension diabetes", 10).unwrap();
        assert_eq!(results, vec![matching.id]);
    }

    #[test]
    fn query_of_only_quotes_returns_empty() {
        // FTS5 phrases cannot contain double quotes; stray quotes are stripped
        // and a query of nothing else must yield an empty (non-error) result.
        let conn = migrated();
        let rec = new_rec_with("q.wav", Some("some transcript"), None);
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "\"\"\"", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn fts_metacharacters_are_treated_literally() {
        // Parentheses/colons/asterisks in user input must not alter FTS
        // semantics or cause parse errors.
        let conn = migrated();
        let rec = new_rec_with("m.wav", Some("assessment (working): plan *deferred*"), None);
        let id = rec.id;
        RecordingsRepo::insert(&conn, &rec).unwrap();

        let results = SearchRepo::search(&conn, "(working):", 10).unwrap();
        assert_eq!(results, vec![id]);
    }
}
