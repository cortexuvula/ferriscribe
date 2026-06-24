use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS vocabulary_entries (
            id              TEXT PRIMARY KEY NOT NULL,
            find_text       TEXT NOT NULL,
            replacement     TEXT NOT NULL,
            category        TEXT NOT NULL DEFAULT 'general',
            case_sensitive  INTEGER NOT NULL DEFAULT 0,
            priority        INTEGER NOT NULL DEFAULT 0,
            enabled         INTEGER NOT NULL DEFAULT 1,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_vocabulary_find_text
            ON vocabulary_entries(find_text);
    "#,
    )?;
    Ok(())
}
