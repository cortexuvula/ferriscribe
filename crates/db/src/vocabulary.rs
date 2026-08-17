//! CRUD operations for the `vocabulary_entries` table.
//!
//! Vocabulary entries are find-and-replace rules applied to transcripts.
//! Each entry maps `find_text` to `replacement`, with support for categories,
//! case sensitivity, priority ordering, and enable/disable toggling.

use rusqlite::Connection;
use uuid::Uuid;

use medical_core::types::vocabulary::{VocabularyCategory, VocabularyEntry};

use crate::{DbError, DbResult};

/// Repository for medical vocabulary substitution rules.
///
/// All methods are associated functions that take a `&Connection`. Entries
/// are ordered by priority (descending) then by `find_text` length
/// (descending, so longer matches take precedence).
pub struct VocabularyRepo;

impl VocabularyRepo {
    /// List all vocabulary entries, ordered by priority DESC then
    /// `find_text` length DESC.
    pub fn list_all(conn: &Connection) -> DbResult<Vec<VocabularyEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at
             FROM vocabulary_entries
             ORDER BY priority DESC, length(find_text) DESC"
        )?;
        let rows = stmt.query_map([], Self::row_to_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// List only enabled vocabulary entries, ordered by priority DESC then
    /// `find_text` length DESC.
    pub fn list_enabled(conn: &Connection) -> DbResult<Vec<VocabularyEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at
             FROM vocabulary_entries
             WHERE enabled = 1
             ORDER BY priority DESC, length(find_text) DESC"
        )?;
        let rows = stmt.query_map([], Self::row_to_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// List entries belonging to a specific category.
    pub fn list_by_category(
        conn: &Connection,
        category: &VocabularyCategory,
    ) -> DbResult<Vec<VocabularyEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at
             FROM vocabulary_entries
             WHERE category = ?1
             ORDER BY priority DESC, length(find_text) DESC"
        )?;
        let rows = stmt.query_map([category.as_str()], Self::row_to_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Fetch a single vocabulary entry by its UUID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if no entry with the given ID exists.
    pub fn get_by_id(conn: &Connection, id: &Uuid) -> DbResult<VocabularyEntry> {
        conn.query_row(
            "SELECT id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at
             FROM vocabulary_entries WHERE id = ?1",
            [id.to_string()],
            Self::row_to_entry,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                DbError::NotFound(format!("vocabulary entry {id}"))
            }
            other => DbError::Sqlite(other),
        })
    }

    /// Insert a new vocabulary entry.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`] on unique
    /// constraint violation (duplicate `find_text`).
    pub fn insert(conn: &Connection, entry: &VocabularyEntry) -> DbResult<()> {
        conn.execute(
            "INSERT INTO vocabulary_entries (id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                entry.id.to_string(),
                entry.find_text,
                entry.replacement,
                entry.category.as_str(),
                entry.case_sensitive as i32,
                entry.priority,
                entry.enabled as i32,
                entry.created_at.to_rfc3339(),
                entry.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Insert or update a vocabulary entry, keyed on `find_text`.
    ///
    /// If an entry with the same `find_text` already exists, its replacement,
    /// category, case sensitivity, priority, enabled flag, and `updated_at`
    /// are overwritten.
    pub fn upsert(conn: &Connection, entry: &VocabularyEntry) -> DbResult<()> {
        conn.execute(
            "INSERT INTO vocabulary_entries (id, find_text, replacement, category, case_sensitive, priority, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(find_text) DO UPDATE SET
                 replacement = excluded.replacement,
                 category = excluded.category,
                 case_sensitive = excluded.case_sensitive,
                 priority = excluded.priority,
                 enabled = excluded.enabled,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                entry.id.to_string(),
                entry.find_text,
                entry.replacement,
                entry.category.as_str(),
                entry.case_sensitive as i32,
                entry.priority,
                entry.enabled as i32,
                entry.created_at.to_rfc3339(),
                entry.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Update all fields of an existing vocabulary entry by ID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if no entry with the given ID exists.
    pub fn update(conn: &Connection, entry: &VocabularyEntry) -> DbResult<()> {
        let rows = conn.execute(
            "UPDATE vocabulary_entries SET find_text = ?1, replacement = ?2, category = ?3, case_sensitive = ?4, priority = ?5, enabled = ?6, updated_at = ?7
             WHERE id = ?8",
            rusqlite::params![
                entry.find_text,
                entry.replacement,
                entry.category.as_str(),
                entry.case_sensitive as i32,
                entry.priority,
                entry.enabled as i32,
                entry.updated_at.to_rfc3339(),
                entry.id.to_string(),
            ],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("vocabulary entry {}", entry.id)));
        }
        Ok(())
    }

    /// Delete a vocabulary entry by ID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if the entry does not exist.
    pub fn delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let rows = conn.execute(
            "DELETE FROM vocabulary_entries WHERE id = ?1",
            [id.to_string()],
        )?;
        if rows == 0 {
            return Err(DbError::NotFound(format!("vocabulary entry {id}")));
        }
        Ok(())
    }

    /// Delete all vocabulary entries. Returns the number of rows removed.
    pub fn delete_all(conn: &Connection) -> DbResult<u32> {
        let rows = conn.execute("DELETE FROM vocabulary_entries", [])?;
        Ok(rows as u32)
    }

    /// Return `(total, enabled)` counts for the vocabulary table.
    pub fn count(conn: &Connection) -> DbResult<(u32, u32)> {
        let total: u32 =
            conn.query_row("SELECT COUNT(*) FROM vocabulary_entries", [], |r| r.get(0))?;
        let enabled: u32 = conn.query_row(
            "SELECT COUNT(*) FROM vocabulary_entries WHERE enabled = 1",
            [],
            |r| r.get(0),
        )?;
        Ok((total, enabled))
    }

    fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<VocabularyEntry> {
        let id_str: String = row.get(0)?;
        let category_str: String = row.get(3)?;
        let case_sensitive_int: i32 = row.get(4)?;
        let enabled_int: i32 = row.get(6)?;
        let created_str: String = row.get(7)?;
        let updated_str: String = row.get(8)?;

        Ok(VocabularyEntry {
            id: Uuid::parse_str(&id_str).map_err(|e| {
                tracing::error!(id_str = %id_str, error = %e, "corrupt vocabulary id in DB");
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            find_text: row.get(1)?,
            replacement: row.get(2)?,
            category: VocabularyCategory::from_str(&category_str),
            case_sensitive: case_sensitive_int != 0,
            priority: row.get(5)?,
            enabled: enabled_int != 0,
            created_at: crate::parse_db_timestamp(
                7,
                &created_str,
                "vocabulary_entries.created_at",
            )?,
            updated_at: crate::parse_db_timestamp(
                8,
                &updated_str,
                "vocabulary_entries.updated_at",
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entry(find_text: &str) -> VocabularyEntry {
        VocabularyEntry {
            id: Uuid::new_v4(),
            find_text: find_text.to_string(),
            replacement: format!("replacement for {find_text}"),
            category: VocabularyCategory::from_str("general"),
            case_sensitive: false,
            priority: 0,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn duplicate_insert_is_flagged_as_unique_violation() {
        let db = crate::Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        VocabularyRepo::insert(&conn, &test_entry("htn")).expect("first insert");
        let err =
            VocabularyRepo::insert(&conn, &test_entry("htn")).expect_err("duplicate rejected");
        assert!(
            err.is_unique_violation(),
            "duplicate find_text must map to a unique violation, got: {err}"
        );
    }
}
