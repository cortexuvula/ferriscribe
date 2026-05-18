//! Per-user dictionary of accepted spellings.
//!
//! Backs the in-app spellchecker: words on this list are never flagged as
//! misspelled. Storage is case-insensitive via a `UNIQUE INDEX` on
//! `LOWER(word)`, so "Lisinopril" and "lisinopril" are the same entry.

use crate::DbResult;
use rusqlite::{Connection, params};

pub struct UserDictionaryRepo;

impl UserDictionaryRepo {
    /// List all dictionary words, sorted case-insensitively.
    pub fn list(conn: &Connection) -> DbResult<Vec<String>> {
        let mut stmt =
            conn.prepare("SELECT word FROM user_dictionary ORDER BY LOWER(word)")?;
        let words = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(words)
    }

    /// Add a word to the dictionary. Whitespace is trimmed; empty input is a
    /// no-op. Returns `true` if a new row was inserted, `false` if the word
    /// (case-insensitive) was already present or input was empty.
    pub fn add(conn: &Connection, word: &str) -> DbResult<bool> {
        let trimmed = word.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let changed = conn.execute(
            "INSERT OR IGNORE INTO user_dictionary (word) VALUES (?1)",
            params![trimmed],
        )?;
        Ok(changed > 0)
    }

    /// Remove a word (case-insensitive match). Returns `true` if a row was
    /// deleted, `false` if no matching word existed.
    pub fn remove(conn: &Connection, word: &str) -> DbResult<bool> {
        let changed = conn.execute(
            "DELETE FROM user_dictionary WHERE LOWER(word) = LOWER(?1)",
            params![word],
        )?;
        Ok(changed > 0)
    }

    /// Check whether a word is present in the dictionary (case-insensitive).
    pub fn contains(conn: &Connection, word: &str) -> DbResult<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM user_dictionary WHERE LOWER(word) = LOWER(?1)",
            params![word],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn add_then_list_returns_word() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "atenolol").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["atenolol"]);
    }

    #[test]
    fn add_is_idempotent_case_insensitive() {
        let conn = fresh();
        assert!(UserDictionaryRepo::add(&conn, "Lisinopril").unwrap());
        assert!(!UserDictionaryRepo::add(&conn, "lisinopril").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn contains_is_case_insensitive() {
        let conn = fresh();
        UserDictionaryRepo::add(&conn, "metformin").unwrap();
        assert!(UserDictionaryRepo::contains(&conn, "METFORMIN").unwrap());
        assert!(UserDictionaryRepo::contains(&conn, "metformin").unwrap());
        assert!(!UserDictionaryRepo::contains(&conn, "unknown").unwrap());
    }

    #[test]
    fn remove_returns_false_when_absent() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::remove(&conn, "ghost").unwrap());
    }

    #[test]
    fn add_strips_whitespace_and_skips_empty() {
        let conn = fresh();
        assert!(!UserDictionaryRepo::add(&conn, "   ").unwrap());
        assert!(UserDictionaryRepo::add(&conn, "  word  ").unwrap());
        assert_eq!(UserDictionaryRepo::list(&conn).unwrap(), vec!["word"]);
    }
}
