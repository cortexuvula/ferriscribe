//! Repository for `generations` (training-corpus capture table).
//!
//! See docs/superpowers/specs/2026-05-11-training-corpus-design.md.
//! Personal use only; data never leaves the device unless the
//! clinician explicitly exports via the (Phase 3) pipeline.

use crate::{DbError, DbResult};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// Inputs needed at row-insertion time. Some fields (final_text,
/// edit_distance, etc.) are NULL at insert and get populated by
/// later updates.
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

pub struct GenerationsRepo;

impl GenerationsRepo {
    /// Insert a new generation row. Computes `regeneration_seq` by
    /// finding the max for the same (recording_id, output_type) and
    /// adding 1; if none exists, starts at 1.
    pub fn record_generation(
        conn: &Connection,
        input: GenerationInsert<'_>,
    ) -> DbResult<Generation> {
        let id = Uuid::new_v4();
        let prev_max: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations \
                 WHERE recording_id = ? AND output_type = ?",
                params![input.recording_id.to_string(), input.output_type],
                |r| r.get(0),
            )
            .unwrap_or(0);
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
        Ok(Generation {
            id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_else(|_| Uuid::nil()),
            recording_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_else(|_| Uuid::nil()),
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

    /// Set `final_text` and `finalized_at` on the most recent
    /// generation row for the given (recording_id, output_type).
    /// Returns the updated row, or `Ok(None)` if no matching row
    /// exists (capture was off when the SOAP was generated).
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

    /// Update the cached edit-distance signals. Called by the
    /// background task that computes word-level Levenshtein.
    /// Safe to call repeatedly (idempotent).
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
}
