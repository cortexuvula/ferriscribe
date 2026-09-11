//! Migration 020: `generation_provenance` table — output-bound input
//! fingerprints for the Generation tab's freshness verdicts.
//!
//! Freshness must never guess: an output is "current" only when the
//! effective inputs used to produce it are provably unchanged. This table
//! stores a canonical SHA-256 digest per (recording, doc type) capturing
//! the effective inputs AT GENERATION TIME:
//!
//! - `input_digest`  — the canonical effective-input digest (live frontend
//!   values + resolved settings + resolved template/pack + source content
//!   as applicable to the type). Rebuilt identically by the read side.
//! - `output_digest` — digest of the generated output text actually
//!   persisted. If the user later edits the stored output by hand, the
//!   binding breaks and freshness must fall back to `unknown`, never
//!   claim current.
//!
//! PHI discipline: only digests and structural metadata (provider/model
//! names, timestamps) are stored here — never context text, never output
//! text. Generation commands (and now freshness reads) share this table.
//!
//! Personal use only; digests never leave the device through this feature.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS generation_provenance (
            id             TEXT PRIMARY KEY NOT NULL,
            recording_id   TEXT NOT NULL,
            doc_type       TEXT NOT NULL,

            created_at     TEXT NOT NULL DEFAULT (datetime('now')),

            ai_provider    TEXT NOT NULL,
            ai_model       TEXT NOT NULL,

            input_digest   TEXT NOT NULL,
            output_digest  TEXT NOT NULL,

            -- For SOAP-derived types (referral, letter): digest of the
            -- soap_note this output was generated FROM. Freshness compares
            -- it against digest(recordings.soap_note) — a hand-edited or
            -- regenerated SOAP flags downstream outputs stale. NULL for
            -- transcript-based types (soap, peer_discussion).
            source_digest  TEXT,

            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_generation_provenance_unique
            ON generation_provenance (recording_id, doc_type);
        CREATE INDEX IF NOT EXISTS idx_generation_provenance_recording
            ON generation_provenance (recording_id);
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
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn provenance_table_exists_after_migration() {
        let conn = migrated();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='generation_provenance'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        assert!(
            exists,
            "generation_provenance table should exist after migration"
        );
    }

    #[test]
    fn provenance_table_has_required_columns() {
        let conn = migrated();
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(generation_provenance)")
                .unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        for required in &[
            "id",
            "recording_id",
            "doc_type",
            "created_at",
            "ai_provider",
            "ai_model",
            "input_digest",
            "output_digest",
            "source_digest",
        ] {
            assert!(
                columns.iter().any(|c| c == required),
                "missing column: {required}; have: {columns:?}"
            );
        }
    }

    #[test]
    fn unique_index_rejects_duplicate_recording_doc_type() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES ('rec1','file.wav','done',datetime('now'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO generation_provenance \
             (id, recording_id, doc_type, ai_provider, ai_model, input_digest, output_digest) \
             VALUES ('p1','rec1','soap','ollama','llama3','in1','out1')",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO generation_provenance \
             (id, recording_id, doc_type, ai_provider, ai_model, input_digest, output_digest) \
             VALUES ('p2','rec1','soap','ollama','llama3','in2','out2')",
            [],
        );
        assert!(
            dup.is_err(),
            "duplicate (recording_id, doc_type) must be rejected"
        );
    }

    #[test]
    fn cascade_delete_removes_provenance() {
        let conn = migrated();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES ('rec1','file.wav','done',datetime('now'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO generation_provenance \
             (id, recording_id, doc_type, ai_provider, ai_model, input_digest, output_digest) \
             VALUES ('p1','rec1','soap','ollama','llama3','in1','out1')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM recordings WHERE id='rec1'", [])
            .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM generation_provenance WHERE id='p1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "provenance should cascade-delete with its recording"
        );
    }
}
