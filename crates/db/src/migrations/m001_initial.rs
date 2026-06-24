//! Initial schema migration — creates all application tables and triggers.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        -- ---------------------------------------------------------------
        -- recordings
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS recordings (
            id                  TEXT PRIMARY KEY NOT NULL,
            filename            TEXT NOT NULL,
            transcript          TEXT,
            soap_note           TEXT,
            referral            TEXT,
            letter              TEXT,
            chat                TEXT,
            patient_name        TEXT,
            audio_path          TEXT,
            duration_seconds    REAL,
            file_size_bytes     INTEGER,
            stt_provider        TEXT,
            ai_provider         TEXT,
            tags                TEXT,
            processing_status   TEXT NOT NULL DEFAULT '"pending"',
            created_at          TEXT NOT NULL DEFAULT (datetime('now')),
            metadata            TEXT DEFAULT 'null'
        );

        CREATE INDEX IF NOT EXISTS idx_recordings_created_at
            ON recordings (created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_recordings_filename
            ON recordings (filename);
        CREATE INDEX IF NOT EXISTS idx_recordings_status
            ON recordings (processing_status);

        -- ---------------------------------------------------------------
        -- FTS5 virtual table for full-text search
        -- ---------------------------------------------------------------
        CREATE VIRTUAL TABLE IF NOT EXISTS recordings_fts USING fts5(
            id UNINDEXED,
            filename,
            transcript,
            soap_note,
            referral,
            letter,
            patient_name,
            content='recordings',
            content_rowid='rowid'
        );

        -- Sync FTS on INSERT
        CREATE TRIGGER IF NOT EXISTS recordings_fts_insert
        AFTER INSERT ON recordings BEGIN
            INSERT INTO recordings_fts (rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
            VALUES (new.rowid, new.id, new.filename, new.transcript, new.soap_note, new.referral, new.letter, new.patient_name);
        END;

        -- Sync FTS on UPDATE
        CREATE TRIGGER IF NOT EXISTS recordings_fts_update
        AFTER UPDATE ON recordings BEGIN
            INSERT INTO recordings_fts (recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
            VALUES ('delete', old.rowid, old.id, old.filename, old.transcript, old.soap_note, old.referral, old.letter, old.patient_name);
            INSERT INTO recordings_fts (rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
            VALUES (new.rowid, new.id, new.filename, new.transcript, new.soap_note, new.referral, new.letter, new.patient_name);
        END;

        -- Sync FTS on DELETE
        CREATE TRIGGER IF NOT EXISTS recordings_fts_delete
        AFTER DELETE ON recordings BEGIN
            INSERT INTO recordings_fts (recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
            VALUES ('delete', old.rowid, old.id, old.filename, old.transcript, old.soap_note, old.referral, old.letter, old.patient_name);
        END;

        -- ---------------------------------------------------------------
        -- settings (key-value store)
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS settings (
            key         TEXT PRIMARY KEY NOT NULL,
            value       TEXT NOT NULL,
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- ---------------------------------------------------------------
        -- audit_log (append-only)
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS audit_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp   TEXT NOT NULL DEFAULT (datetime('now')),
            action      TEXT NOT NULL,
            actor       TEXT NOT NULL,
            resource    TEXT NOT NULL,
            details     TEXT
        );

        -- Prevent UPDATE on audit_log
        CREATE TRIGGER IF NOT EXISTS audit_log_no_update
        BEFORE UPDATE ON audit_log BEGIN
            SELECT RAISE(ABORT, 'audit_log rows are immutable');
        END;

        -- Prevent DELETE on audit_log
        CREATE TRIGGER IF NOT EXISTS audit_log_no_delete
        BEFORE DELETE ON audit_log BEGIN
            SELECT RAISE(ABORT, 'audit_log rows cannot be deleted');
        END;

        -- ---------------------------------------------------------------
        -- saved_recipients
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS saved_recipients (
            id              TEXT PRIMARY KEY NOT NULL,
            name            TEXT NOT NULL,
            title           TEXT,
            specialty       TEXT,
            organization    TEXT,
            address         TEXT,
            city            TEXT,
            state           TEXT,
            postal_code     TEXT,
            phone           TEXT,
            fax             TEXT,
            email           TEXT,
            notes           TEXT,
            category        TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- ---------------------------------------------------------------
        -- processing_queue
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS processing_queue (
            id              TEXT PRIMARY KEY NOT NULL,
            recording_id    TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
            task_type       TEXT NOT NULL,
            priority        INTEGER NOT NULL DEFAULT 0,
            status          TEXT NOT NULL DEFAULT 'pending',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            started_at      TEXT,
            completed_at    TEXT,
            error_count     INTEGER NOT NULL DEFAULT 0,
            last_error      TEXT,
            result          TEXT,
            batch_id        TEXT
        );

        -- ---------------------------------------------------------------
        -- batch_processing
        -- ---------------------------------------------------------------
        CREATE TABLE IF NOT EXISTS batch_processing (
            id              TEXT PRIMARY KEY NOT NULL,
            total_count     INTEGER NOT NULL DEFAULT 0,
            completed_count INTEGER NOT NULL DEFAULT 0,
            failed_count    INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            started_at      TEXT,
            completed_at    TEXT,
            status          TEXT NOT NULL DEFAULT 'pending'
        );
        "#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn in_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn creates_all_tables() {
        let conn = in_memory();
        super::up(&conn).expect("migration should succeed");

        // Verify that every expected table exists
        let tables = [
            "recordings",
            "recordings_fts",
            "settings",
            "audit_log",
            "saved_recipients",
            "processing_queue",
            "batch_processing",
        ];
        for table in tables {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','shadow') AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            assert!(count > 0, "table '{table}' should exist");
        }
    }

    #[test]
    fn audit_rejects_updates() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        conn.execute(
            "INSERT INTO audit_log (action, actor, resource) VALUES ('test', 'user', 'res')",
            [],
        )
        .expect("insert");
        let result = conn.execute("UPDATE audit_log SET action='hacked' WHERE id=1", []);
        assert!(result.is_err(), "UPDATE on audit_log must be rejected");
    }

    #[test]
    fn audit_rejects_deletes() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        conn.execute(
            "INSERT INTO audit_log (action, actor, resource) VALUES ('test', 'user', 'res')",
            [],
        )
        .expect("insert");
        let result = conn.execute("DELETE FROM audit_log WHERE id=1", []);
        assert!(result.is_err(), "DELETE on audit_log must be rejected");
    }

    #[test]
    fn fts_table_exists() {
        let conn = in_memory();
        super::up(&conn).expect("migration");
        // FTS5 tables show up in sqlite_master with type='table'
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='recordings_fts'",
                [],
                |r| r.get(0),
            )
            .expect("query");
        assert!(count > 0, "recordings_fts should exist");
    }
}
