//! Migration 004: `generations` table for the training-corpus feature.
//!
//! Captures (transcript, AI draft, clinician final) triples. See
//! docs/superpowers/specs/2026-05-11-training-corpus-design.md for
//! the data model rationale. Personal-use-only; no PHI ever leaves
//! this device.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS generations (
            id                    TEXT PRIMARY KEY NOT NULL,
            recording_id          TEXT NOT NULL,
            output_type           TEXT NOT NULL,

            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            finalized_at          TEXT,

            ai_provider           TEXT NOT NULL,
            ai_model              TEXT NOT NULL,
            prompt_template_name  TEXT,

            input_transcript      TEXT NOT NULL,
            input_context_json    TEXT,

            draft_text            TEXT NOT NULL,
            final_text            TEXT,

            corpus_status         TEXT NOT NULL DEFAULT 'candidate'
                CHECK (corpus_status IN ('candidate','promoted','rejected','excluded')),
            corpus_curated_at     TEXT,

            edit_distance         INTEGER,
            edit_ratio            REAL,

            regeneration_seq      INTEGER NOT NULL DEFAULT 1,

            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_generations_recording
            ON generations (recording_id);
        CREATE INDEX IF NOT EXISTS idx_generations_corpus_status
            ON generations (corpus_status, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_generations_created
            ON generations (created_at DESC);
        "#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Enable foreign keys for cascade-delete test.
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn generations_table_exists_after_migration() {
        let conn = migrated();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='generations'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        assert!(exists, "generations table should exist after migration");
    }

    #[test]
    fn generations_table_has_required_columns() {
        let conn = migrated();
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(generations)").unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        for required in &[
            "id",
            "recording_id",
            "output_type",
            "created_at",
            "finalized_at",
            "ai_provider",
            "ai_model",
            "prompt_template_name",
            "input_transcript",
            "input_context_json",
            "draft_text",
            "final_text",
            "corpus_status",
            "corpus_curated_at",
            "edit_distance",
            "edit_ratio",
            "regeneration_seq",
        ] {
            assert!(
                columns.iter().any(|c| c == required),
                "missing column: {required}; have: {columns:?}"
            );
        }
    }

    #[test]
    fn cascade_delete_removes_generations() {
        let conn = migrated();
        // Insert a parent recording first (FK requires it). Use minimal
        // columns since the schema allows null on most.
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES ('rec1','file.wav','done',datetime('now'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO generations (id, recording_id, output_type, ai_provider, \
                ai_model, input_transcript, draft_text) \
             VALUES ('gen1','rec1','soap','ollama','llama3','t','d')",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM recordings WHERE id='rec1'", [])
            .unwrap();

        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM generations WHERE id='gen1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "generation should cascade-delete with its parent recording"
        );
    }
}
