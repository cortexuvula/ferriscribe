//! Repository for `generation_provenance` — output-bound effective-input
//! digests backing the Generation tab's freshness verdicts.
//!
//! One row per `(recording_id, doc_type)` (unique index): the digest of the
//! effective inputs used by the LAST generation of that type, plus the
//! digest of the output it produced. Written atomically by generation
//! commands alongside the output persist; read (read-only, lock-free) by
//! `get_generation_freshness`.
//!
//! **PHI discipline:** rows carry digests and provider/model names only —
//! never context text, output text, or patient identifiers.

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::{DbError, DbResult};

/// A provenance row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationProvenance {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub doc_type: String,
    pub created_at: String,
    pub ai_provider: String,
    pub ai_model: String,
    pub input_digest: String,
    pub output_digest: String,
    /// SOAP-source binding for derived types; NULL for soap/peer_discussion.
    pub source_digest: Option<String>,
}

/// Values needed when recording provenance for one output.
#[derive(Debug, Clone)]
pub struct ProvenanceInsert {
    pub recording_id: Uuid,
    pub doc_type: &'static str,
    pub ai_provider: String,
    pub ai_model: String,
    pub input_digest: String,
    pub output_digest: String,
    pub source_digest: Option<String>,
}

pub struct GenerationProvenanceRepo;

impl GenerationProvenanceRepo {
    /// Upsert the provenance row for `(recording_id, doc_type)`.
    ///
    /// Generation is serialized per recording by the generation lock, so a
    /// plain `INSERT OR REPLACE` on the unique `(recording_id, doc_type)`
    /// index is sufficient — the newest generation for a type wins, which
    /// is exactly the binding freshness must reflect.
    pub fn upsert(conn: &Connection, input: &ProvenanceInsert) -> DbResult<()> {
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT OR REPLACE INTO generation_provenance
                (id, recording_id, doc_type, ai_provider, ai_model,
                 input_digest, output_digest, source_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.to_string(),
                input.recording_id.to_string(),
                input.doc_type,
                input.ai_provider,
                input.ai_model,
                input.input_digest,
                input.output_digest,
                input.source_digest,
            ],
        )?;
        Ok(())
    }

    /// Fetch the provenance row for `(recording_id, doc_type)`, if any.
    pub fn get(
        conn: &Connection,
        recording_id: Uuid,
        doc_type: &str,
    ) -> DbResult<Option<GenerationProvenance>> {
        conn.query_row(
            "SELECT id, recording_id, doc_type, created_at,
                    ai_provider, ai_model, input_digest, output_digest,
                    source_digest
             FROM generation_provenance
             WHERE recording_id = ?1 AND doc_type = ?2",
            params![recording_id.to_string(), doc_type],
            Self::row_to_provenance,
        )
        .optional()
        .map_err(DbError::from)
    }

    fn row_to_provenance(row: &rusqlite::Row) -> rusqlite::Result<GenerationProvenance> {
        let id_str: String = row.get(0)?;
        let id = Uuid::parse_str(&id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let rec_str: String = row.get(1)?;
        let recording_id = Uuid::parse_str(&rec_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(GenerationProvenance {
            id,
            recording_id,
            doc_type: row.get(2)?,
            created_at: row.get(3)?,
            ai_provider: row.get(4)?,
            ai_model: row.get(5)?,
            input_digest: row.get(6)?,
            output_digest: row.get(7)?,
            source_digest: row.get(8)?,
        })
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

    fn seed_recording(conn: &Connection) -> Uuid {
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO recordings (id, filename, processing_status, created_at) \
             VALUES (?1, 'test.wav', 'done', datetime('now'))",
            params![id.to_string()],
        )
        .unwrap();
        id
    }

    fn insert(recording_id: Uuid, doc_type: &'static str, input_digest: &str) -> ProvenanceInsert {
        ProvenanceInsert {
            recording_id,
            doc_type,
            ai_provider: "ollama".into(),
            ai_model: "llama3".into(),
            input_digest: input_digest.into(),
            output_digest: format!("out-{input_digest}"),
            source_digest: None,
        }
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let conn = migrated();
        let rec = seed_recording(&conn);
        GenerationProvenanceRepo::upsert(&conn, &insert(rec, "soap", "abc123")).unwrap();
        let row = GenerationProvenanceRepo::get(&conn, rec, "soap")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.recording_id, rec);
        assert_eq!(row.input_digest, "abc123");
        assert_eq!(row.output_digest, "out-abc123");
        assert_eq!(row.ai_provider, "ollama");
    }

    #[test]
    fn upsert_replaces_previous_row_for_same_type() {
        let conn = migrated();
        let rec = seed_recording(&conn);
        GenerationProvenanceRepo::upsert(&conn, &insert(rec, "soap", "old")).unwrap();
        GenerationProvenanceRepo::upsert(&conn, &insert(rec, "soap", "new")).unwrap();
        let row = GenerationProvenanceRepo::get(&conn, rec, "soap")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.input_digest, "new", "newest generation wins");
        // Still exactly one row for the pair.
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM generation_provenance WHERE recording_id = ?1",
                params![rec.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn doc_types_are_independent() {
        let conn = migrated();
        let rec = seed_recording(&conn);
        GenerationProvenanceRepo::upsert(&conn, &insert(rec, "soap", "s")).unwrap();
        GenerationProvenanceRepo::upsert(&conn, &insert(rec, "letter", "l")).unwrap();
        assert_eq!(
            GenerationProvenanceRepo::get(&conn, rec, "soap")
                .unwrap()
                .unwrap()
                .input_digest,
            "s"
        );
        assert_eq!(
            GenerationProvenanceRepo::get(&conn, rec, "letter")
                .unwrap()
                .unwrap()
                .input_digest,
            "l"
        );
        assert!(
            GenerationProvenanceRepo::get(&conn, rec, "referral")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn get_returns_none_for_unknown_recording() {
        let conn = migrated();
        assert!(
            GenerationProvenanceRepo::get(&conn, Uuid::new_v4(), "soap")
                .unwrap()
                .is_none()
        );
    }
}
