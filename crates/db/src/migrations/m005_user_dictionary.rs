//! Migration 005: `user_dictionary` table for the in-app spellcheck wordlist.
//!
//! Per-user list of accepted spellings. Words on this list are not flagged by
//! the in-app spellchecker. See docs/superpowers/plans/2026-05-18-spellcheck.md.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS user_dictionary (
            id          INTEGER PRIMARY KEY,
            word        TEXT NOT NULL,
            added_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_user_dictionary_word_nocase
            ON user_dictionary (LOWER(word));
        "#,
    )?;
    Ok(())
}
