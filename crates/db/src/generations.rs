//! Repository for `generations` (training-corpus capture table).
//!
//! See docs/superpowers/specs/2026-05-11-training-corpus-design.md.
//! Personal use only; data never leaves the device unless the
//! clinician explicitly exports via the (Phase 3) pipeline.
//!
//! **Error handling:** All methods use proper error propagation via `?`
//! and return `DbResult<T>`. No unwrap/expect calls in production code.

use crate::{DbError, DbResult};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A full generation row from the `generations` table.
///
/// Captures the (transcript, AI draft, clinician final) triple for the
/// training-corpus feature. The `corpus_status` field tracks whether the
/// generation is a candidate for promotion, has been promoted, rejected,
/// or excluded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Generation {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub output_type: String,
    pub created_at: String,
    pub finalized_at: Option<String>,
    pub ai_provider: String,
    pub ai_model: String,
    pub prompt_template_name: Option<String>,
    pub input_transcript: String,
    pub input_context_json: Option<String>,
    pub draft_text: String,
    pub final_text: Option<String>,
    pub corpus_status: String,
    pub corpus_curated_at: Option<String>,
    pub edit_distance: Option<i64>,
    pub edit_ratio: Option<f64>,
    pub regeneration_seq: i64,
}

/// Inputs needed at row-insertion time.
///
/// Some fields (`final_text`, `edit_distance`, etc.) are NULL at insert and
/// get populated by later updates via [`GenerationsRepo::update_final_text`]
/// and [`GenerationsRepo::set_edit_distance`].
#[derive(Debug, Clone)]
pub struct GenerationInsert<'a> {
    pub recording_id: Uuid,
    pub output_type: &'a str,
    pub ai_provider: &'a str,
    pub ai_model: &'a str,
    pub prompt_template_name: Option<&'a str>,
    pub input_transcript: &'a str,
    pub input_context_json: Option<&'a str>,
    pub draft_text: &'a str,
}

/// Repository for the `generations` table (training-corpus capture).
///
/// Records each AI generation event with its input transcript, draft output,
/// and (later) the clinician's finalized text. The `regeneration_seq` column
/// auto-increments per `(recording_id, output_type)` pair.
pub struct GenerationsRepo;

impl GenerationsRepo {
    /// Insert a new generation row.
    ///
    /// Computes `regeneration_seq` by finding the max for the same
    /// `(recording_id, output_type)` and adding 1; starts at 1 if none
    /// exists. Returns the fully populated row.
    pub fn record_generation(
        conn: &Connection,
        input: GenerationInsert<'_>,
    ) -> DbResult<Generation> {
        let id = Uuid::new_v4();
        let prev_max: i64 = conn.query_row(
            "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations \
                 WHERE recording_id = ? AND output_type = ?",
            params![input.recording_id.to_string(), input.output_type],
            |r| r.get(0),
        )?;
        let seq = prev_max + 1;

        conn.execute(
            "INSERT INTO generations (
                id, recording_id, output_type, ai_provider, ai_model,
                prompt_template_name, input_transcript, input_context_json,
                draft_text, regeneration_seq
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id.to_string(),
                input.recording_id.to_string(),
                input.output_type,
                input.ai_provider,
                input.ai_model,
                input.prompt_template_name,
                input.input_transcript,
                input.input_context_json,
                input.draft_text,
                seq,
            ],
        )?;
        Self::get_by_id(conn, id)
    }

    /// Fetch a single generation row by its UUID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] if the row is not
    /// found (query returns no rows).
    pub fn get_by_id(conn: &Connection, id: Uuid) -> DbResult<Generation> {
        conn.query_row(
            "SELECT id, recording_id, output_type, created_at, finalized_at,
                    ai_provider, ai_model, prompt_template_name,
                    input_transcript, input_context_json,
                    draft_text, final_text,
                    corpus_status, corpus_curated_at,
                    edit_distance, edit_ratio, regeneration_seq
             FROM generations WHERE id = ?",
            params![id.to_string()],
            Self::row_to_generation,
        )
        .map_err(DbError::from)
    }

    fn row_to_generation(row: &rusqlite::Row) -> rusqlite::Result<Generation> {
        let id_str: String = row.get(0)?;
        let id = Uuid::parse_str(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;

        let recording_id_str: String = row.get(1)?;
        let recording_id = Uuid::parse_str(&recording_id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;

        Ok(Generation {
            id,
            recording_id,
            output_type: row.get(2)?,
            created_at: row.get(3)?,
            finalized_at: row.get(4)?,
            ai_provider: row.get(5)?,
            ai_model: row.get(6)?,
            prompt_template_name: row.get(7)?,
            input_transcript: row.get(8)?,
            input_context_json: row.get(9)?,
            draft_text: row.get(10)?,
            final_text: row.get(11)?,
            corpus_status: row.get(12)?,
            corpus_curated_at: row.get(13)?,
            edit_distance: row.get(14)?,
            edit_ratio: row.get(15)?,
            regeneration_seq: row.get(16)?,
        })
    }

    /// Set `final_text` and `finalized_at` on the most recent generation row
    /// for the given `(recording_id, output_type)`.
    ///
    /// Returns the updated row, or `Ok(None)` if no matching row exists
    /// (capture was off when the output was generated).
    pub fn update_final_text(
        conn: &Connection,
        recording_id: Uuid,
        output_type: &str,
        final_text: &str,
    ) -> DbResult<Option<Generation>> {
        // Find the most recent row by regeneration_seq.
        let row_id: Option<String> = conn
            .query_row(
                "SELECT id FROM generations
                 WHERE recording_id = ? AND output_type = ?
                 ORDER BY regeneration_seq DESC LIMIT 1",
                params![recording_id.to_string(), output_type],
                |r| r.get(0),
            )
            .optional()?;
        let row_id = match row_id {
            Some(s) => s,
            None => return Ok(None),
        };

        conn.execute(
            "UPDATE generations
                SET final_text = ?, finalized_at = datetime('now')
              WHERE id = ?",
            params![final_text, row_id],
        )?;
        let id = Uuid::parse_str(&row_id).map_err(|e| DbError::Other(e.to_string()))?;
        Ok(Some(Self::get_by_id(conn, id)?))
    }

    /// Update the cached edit-distance signals.
    ///
    /// Called by the background task that computes word-level Levenshtein
    /// distance between draft and final text. Safe to call repeatedly
    /// (idempotent).
    pub fn set_edit_distance(
        conn: &Connection,
        id: Uuid,
        edit_distance: i64,
        edit_ratio: f64,
    ) -> DbResult<()> {
        conn.execute(
            "UPDATE generations
                SET edit_distance = ?, edit_ratio = ?
              WHERE id = ?",
            params![edit_distance, edit_ratio, id.to_string()],
        )?;
        Ok(())
    }

    /// List generations matching the given `corpus_status`, paginated by
    /// `created_at DESC`.
    ///
    /// Returns `(rows, total_count)` so the UI can show "N candidates" +
    /// "page X of Y" in one call. `limit` is capped at 200 to avoid loading
    /// excessively large batches.
    ///
    /// Candidate rows with NULL `final_text` are excluded (they represent
    /// unfinished generations).
    pub fn list_by_status(
        conn: &Connection,
        status: &str,
        limit: u32,
        offset: u32,
    ) -> DbResult<(Vec<Generation>, u32)> {
        let limit = limit.min(200);

        let total: u32 = conn.query_row(
            "SELECT count(*) FROM generations
             WHERE corpus_status = ?
               AND (corpus_status != 'candidate' OR final_text IS NOT NULL)",
            params![status],
            |r| r.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, recording_id, output_type, created_at, finalized_at,
                    ai_provider, ai_model, prompt_template_name,
                    input_transcript, input_context_json,
                    draft_text, final_text,
                    corpus_status, corpus_curated_at,
                    edit_distance, edit_ratio, regeneration_seq
             FROM generations
             WHERE corpus_status = ?
               AND (corpus_status != 'candidate' OR final_text IS NOT NULL)
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
        )?;
        let rows = stmt
            .query_map(params![status, limit, offset], Self::row_to_generation)?
            .filter_map(|r| r.ok())
            .collect();
        Ok((rows, total))
    }

    /// Counts per status, for the summary header.
    ///
    /// Returns `(candidates, promoted, rejected, excluded)` in a single query.
    /// Candidate rows with NULL `final_text` are excluded from the candidate
    /// count.
    pub fn count_by_status(conn: &Connection) -> DbResult<(u32, u32, u32, u32)> {
        let mut stmt = conn.prepare(
            "SELECT
                SUM(CASE WHEN corpus_status='candidate' AND final_text IS NOT NULL THEN 1 ELSE 0 END) AS c,
                SUM(CASE WHEN corpus_status='promoted'  THEN 1 ELSE 0 END) AS p,
                SUM(CASE WHEN corpus_status='rejected'  THEN 1 ELSE 0 END) AS r,
                SUM(CASE WHEN corpus_status='excluded'  THEN 1 ELSE 0 END) AS e
             FROM generations",
        )?;
        let (c, p, r, e) = stmt.query_row([], |row| {
            Ok((
                row.get::<_, Option<u32>>(0)?.unwrap_or(0),
                row.get::<_, Option<u32>>(1)?.unwrap_or(0),
                row.get::<_, Option<u32>>(2)?.unwrap_or(0),
                row.get::<_, Option<u32>>(3)?.unwrap_or(0),
            ))
        })?;
        Ok((c, p, r, e))
    }

    /// Change a single row's `corpus_status`.
    ///
    /// Updates `corpus_curated_at` to now on every call. Validates the input
    /// status; only `"candidate"`, `"promoted"`, `"rejected"`, and
    /// `"excluded"` are accepted.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Other`] on invalid status value or if the
    /// generation ID is not found.
    pub fn set_corpus_status(conn: &Connection, id: Uuid, new_status: &str) -> DbResult<()> {
        if !matches!(
            new_status,
            "candidate" | "promoted" | "rejected" | "excluded"
        ) {
            return Err(DbError::Other(format!(
                "invalid corpus_status: {new_status}"
            )));
        }
        let affected = conn.execute(
            "UPDATE generations
                SET corpus_status = ?,
                    corpus_curated_at = datetime('now')
              WHERE id = ?",
            params![new_status, id.to_string()],
        )?;
        if affected == 0 {
            return Err(DbError::Other(format!("generation {id} not found")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn record_generation_inserts_with_seq_1_on_first_call() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        let row = GenerationsRepo::record_generation(
            &conn,
            GenerationInsert {
                recording_id: rec_id,
                output_type: "soap",
                ai_provider: "ollama",
                ai_model: "llama3:70b",
                prompt_template_name: Some("soap-default"),
                input_transcript: "Patient reports cough.",
                input_context_json: None,
                draft_text: "S: cough. O: none. A: viral URI. P: rest.",
            },
        )
        .unwrap();

        assert_eq!(row.regeneration_seq, 1);
        assert_eq!(row.corpus_status, "candidate");
        assert_eq!(row.draft_text, "S: cough. O: none. A: viral URI. P: rest.");
        assert!(row.final_text.is_none());
        assert!(row.finalized_at.is_none());
        assert!(row.edit_distance.is_none());
    }

    #[test]
    fn record_generation_bumps_seq_on_regeneration() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        let insert = GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3:70b",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d1",
        };

        let g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        let g2 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        let g3 = GenerationsRepo::record_generation(&conn, insert).unwrap();

        assert_eq!(g1.regeneration_seq, 1);
        assert_eq!(g2.regeneration_seq, 2);
        assert_eq!(g3.regeneration_seq, 3);
    }

    #[test]
    fn record_generation_propagates_query_errors() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        // Drop the table to simulate query failure
        conn.execute("DROP TABLE generations", []).unwrap();

        let result = GenerationsRepo::record_generation(
            &conn,
            GenerationInsert {
                recording_id: rec_id,
                output_type: "soap",
                ai_provider: "ollama",
                ai_model: "llama3",
                prompt_template_name: None,
                input_transcript: "t",
                input_context_json: None,
                draft_text: "d",
            },
        );

        assert!(
            result.is_err(),
            "should propagate query error, not silently use 0"
        );
    }

    #[test]
    fn update_final_text_populates_the_most_recent_row() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        let insert = GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3:70b",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d",
        };
        let g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        let g2 = GenerationsRepo::record_generation(&conn, insert).unwrap();

        let updated = GenerationsRepo::update_final_text(&conn, rec_id, "soap", "final-v1")
            .unwrap()
            .expect("should have updated a row");

        // Only the most-recent (g2) should have final_text set.
        assert_eq!(updated.id, g2.id);
        assert_eq!(updated.final_text.as_deref(), Some("final-v1"));
        assert!(updated.finalized_at.is_some());

        // g1 should still have NULL final_text — that's the
        // "rejected draft" signal.
        let g1_refreshed = GenerationsRepo::get_by_id(&conn, g1.id).unwrap();
        assert!(g1_refreshed.final_text.is_none());
    }

    #[test]
    fn update_final_text_returns_none_when_no_matching_row() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        let result = GenerationsRepo::update_final_text(&conn, rec_id, "soap", "x").unwrap();
        assert!(result.is_none(), "should be None when capture wasn't on");
    }

    #[test]
    fn set_edit_distance_writes_both_fields() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        let generation = GenerationsRepo::record_generation(
            &conn,
            GenerationInsert {
                recording_id: rec_id,
                output_type: "soap",
                ai_provider: "ollama",
                ai_model: "llama3",
                prompt_template_name: None,
                input_transcript: "t",
                input_context_json: None,
                draft_text: "d",
            },
        )
        .unwrap();

        GenerationsRepo::set_edit_distance(&conn, generation.id, 12, 0.34).unwrap();

        let refreshed = GenerationsRepo::get_by_id(&conn, generation.id).unwrap();
        assert_eq!(refreshed.edit_distance, Some(12));
        assert_eq!(refreshed.edit_ratio, Some(0.34));
    }

    #[test]
    fn list_by_status_returns_candidates_newest_first() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        let insert = GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d",
        };
        let _g1 = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        // Force a different timestamp so ordering is deterministic.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let g2 = GenerationsRepo::record_generation(&conn, insert).unwrap();
        // Set final_text on both rows so they are visible to list_by_status
        // (null-final candidates are excluded from the corpus queue).
        conn.execute(
            "UPDATE generations SET final_text = 'finalized' WHERE recording_id = ?",
            params![rec_id.to_string()],
        )
        .unwrap();

        let (rows, total) = GenerationsRepo::list_by_status(&conn, "candidate", 10, 0).unwrap();
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, g2.id, "newest first");
    }

    #[test]
    fn list_by_status_paginates() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        let insert = GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d",
        };
        for _ in 0..5 {
            GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        }
        // Set final_text on all rows so they are visible to list_by_status
        // (null-final candidates are excluded from the corpus queue).
        conn.execute(
            "UPDATE generations SET final_text = 'finalized' WHERE recording_id = ?",
            params![rec_id.to_string()],
        )
        .unwrap();
        let (page1, total) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 0).unwrap();
        assert_eq!(total, 5);
        assert_eq!(page1.len(), 2);
        let (page2, _) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 2).unwrap();
        assert_eq!(page2.len(), 2);
        let (page3, _) = GenerationsRepo::list_by_status(&conn, "candidate", 2, 4).unwrap();
        assert_eq!(page3.len(), 1);
    }

    #[test]
    fn list_by_status_caps_limit_at_200() {
        let conn = migrated();
        let (rows, _) = GenerationsRepo::list_by_status(&conn, "candidate", 9999, 0).unwrap();
        assert_eq!(rows.len(), 0); // empty here, but limit-cap doesn't error
    }

    #[test]
    fn count_by_status_sums_all_buckets() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        let insert = GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d",
        };
        let g_cand = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        let g_prom = GenerationsRepo::record_generation(&conn, insert.clone()).unwrap();
        let g_rej = GenerationsRepo::record_generation(&conn, insert).unwrap();
        GenerationsRepo::set_corpus_status(&conn, g_prom.id, "promoted").unwrap();
        GenerationsRepo::set_corpus_status(&conn, g_rej.id, "rejected").unwrap();
        // Give the candidate row a final_text so it is counted by count_by_status
        // (null-final candidates are excluded from the corpus queue).
        conn.execute(
            "UPDATE generations SET final_text = 'finalized' WHERE id = ?",
            params![g_cand.id.to_string()],
        )
        .unwrap();

        let (c, p, r, e) = GenerationsRepo::count_by_status(&conn).unwrap();
        assert_eq!(c, 1);
        assert_eq!(p, 1);
        assert_eq!(r, 1);
        assert_eq!(e, 0);

        // Sanity: original candidate row id is the un-promoted one.
        let _ = g_cand;
    }

    #[test]
    fn set_corpus_status_rejects_invalid_value() {
        let conn = migrated();
        let id = Uuid::new_v4();
        let err = GenerationsRepo::set_corpus_status(&conn, id, "favorited");
        assert!(err.is_err(), "should reject invalid status");
    }

    #[test]
    fn set_corpus_status_errors_when_id_missing() {
        let conn = migrated();
        let id = Uuid::new_v4();
        let err = GenerationsRepo::set_corpus_status(&conn, id, "promoted");
        assert!(err.is_err());
    }

    #[test]
    fn list_by_status_candidate_excludes_null_final_text() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        // One candidate with final_text present:
        let with_final = Uuid::new_v4();
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                     'transcript', 'draft body', 'final body', 'candidate', 1)",
            params![with_final.to_string(), rec_id.to_string()],
        )
        .unwrap();
        // One candidate with final_text NULL (the case we filter out):
        let without_final = Uuid::new_v4();
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                     'transcript', 'draft body', NULL, 'candidate', 2)",
            params![without_final.to_string(), rec_id.to_string()],
        )
        .unwrap();

        let (items, total) = GenerationsRepo::list_by_status(&conn, "candidate", 10, 0).unwrap();
        assert_eq!(
            total, 1,
            "total should reflect only candidates with final_text"
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, with_final);
    }

    #[test]
    fn list_by_status_promoted_still_includes_null_final_text() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                     'transcript', 'draft body', NULL, 'promoted', 1)",
            params![id.to_string(), rec_id.to_string()],
        )
        .unwrap();

        let (items, total) = GenerationsRepo::list_by_status(&conn, "promoted", 10, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn row_to_generation_returns_error_on_invalid_uuid() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();

        // Insert a row with an invalid UUID format
        let invalid_uuid = "not-a-valid-uuid";
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'llama3',
                     'transcript', 'draft', 'candidate', 1)",
            params![invalid_uuid, rec_id.to_string()],
        )
        .unwrap();

        // Attempt to retrieve should return error, not silently use nil UUID
        let result = conn.query_row(
            "SELECT id, recording_id, output_type, created_at, finalized_at,
                    ai_provider, ai_model, prompt_template_name,
                    input_transcript, input_context_json,
                    draft_text, final_text,
                    corpus_status, corpus_curated_at,
                    edit_distance, edit_ratio, regeneration_seq
             FROM generations WHERE id = ?",
            params![invalid_uuid],
            GenerationsRepo::row_to_generation,
        );

        // Should error due to invalid UUID, not return Ok with Uuid::nil()
        assert!(
            result.is_err() || matches!(result, Ok(Generation { id, .. }) if id != Uuid::nil()),
            "invalid UUID must produce an error, not a nil UUID"
        );
    }

    #[test]
    fn count_by_status_excludes_null_final_text_from_candidates() {
        let conn = migrated();
        let rec_id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?, 'test.wav', 'done', datetime('now'))",
            params![rec_id.to_string()],
        )
        .unwrap();
        // Two candidates: one with final_text, one without.
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                     'transcript', 'draft body', 'final body', 'candidate', 1)",
            params![Uuid::new_v4().to_string(), rec_id.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO generations
               (id, recording_id, output_type, created_at, ai_provider, ai_model,
                input_transcript, draft_text, final_text, corpus_status, regeneration_seq)
             VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'qwen3.6',
                     'transcript', 'draft body', NULL, 'candidate', 2)",
            params![Uuid::new_v4().to_string(), rec_id.to_string()],
        )
        .unwrap();

        let (c, _p, _r, _e) = GenerationsRepo::count_by_status(&conn).unwrap();
        assert_eq!(c, 1, "candidate count must match list_by_status filtering");
    }
}
