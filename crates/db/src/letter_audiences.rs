//! CRUD operations for the `letter_audiences` table.
//!
//! Letter audiences define the target recipient type for generated letters
//! (e.g. Patient, Insurance Company, Specialist). Each audience carries a
//! system prompt and optional user template. Six built-in audiences are
//! seeded by migration `m006` and cannot be deleted.

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use medical_core::types::letter_audience::LetterAudience;

use crate::{DbError, DbResult};

/// Repository for letter audience definitions.
///
/// Built-in audiences (seeded by migration) are protected from deletion.
/// Custom audiences can be freely created, updated, and removed.
pub struct LetterAudiencesRepo;

impl LetterAudiencesRepo {
    /// List all audiences, built-ins first, then alphabetically by name.
    pub fn list_all(conn: &Connection) -> DbResult<Vec<LetterAudience>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, system_prompt, user_template, is_builtin, created_at, updated_at
             FROM letter_audiences
             ORDER BY is_builtin DESC, name",
        )?;
        let rows = stmt.query_map([], Self::row_to_audience)?;
        let mut audiences = Vec::new();
        for row in rows {
            audiences.push(row?);
        }
        Ok(audiences)
    }

    /// Fetch a single audience by its UUID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::NotFound`] if no audience with the given ID exists.
    pub fn get_by_id(conn: &Connection, id: &Uuid) -> DbResult<LetterAudience> {
        conn.query_row(
            "SELECT id, name, system_prompt, user_template, is_builtin, created_at, updated_at
             FROM letter_audiences WHERE id = ?1",
            [id.to_string()],
            Self::row_to_audience,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                DbError::NotFound(format!("letter audience {id}"))
            }
            other => DbError::Sqlite(other),
        })
    }

    /// Insert or update an audience. Keyed on `id`.
    ///
    /// On conflict, updates name, system prompt, user template, and
    /// `updated_at`. The `is_builtin` and `created_at` fields are preserved.
    pub fn upsert(conn: &Connection, audience: &LetterAudience) -> DbResult<()> {
        conn.execute(
            "INSERT INTO letter_audiences (id, name, system_prompt, user_template, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 system_prompt = excluded.system_prompt,
                 user_template = excluded.user_template,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                audience.id.to_string(),
                audience.name,
                audience.system_prompt,
                audience.user_template,
                audience.is_builtin as i32,
                audience.created_at.to_rfc3339(),
                audience.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Delete a custom audience by ID.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Constraint`] if the audience is built-in
    /// (`is_builtin = 1`), or [`DbError::NotFound`] if it does not exist.
    pub fn delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let rows = conn.execute(
            "DELETE FROM letter_audiences WHERE id = ?1 AND is_builtin = 0",
            [id.to_string()],
        )?;
        if rows == 0 {
            // Check if it's built-in (exists but can't be deleted) or truly not found
            let is_builtin: Option<i32> = conn
                .query_row(
                    "SELECT is_builtin FROM letter_audiences WHERE id = ?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(DbError::Sqlite)?;
            match is_builtin {
                Some(1) => Err(DbError::Constraint(format!(
                    "cannot delete built-in letter audience {id}"
                ))),
                _ => Err(DbError::NotFound(format!("letter audience {id}"))),
            }
        } else {
            Ok(())
        }
    }

    fn row_to_audience(row: &rusqlite::Row<'_>) -> rusqlite::Result<LetterAudience> {
        let id_str: String = row.get(0)?;
        let is_builtin_int: i32 = row.get(4)?;
        let created_str: String = row.get(5)?;
        let updated_str: String = row.get(6)?;

        Ok(LetterAudience {
            id: Uuid::parse_str(&id_str).map_err(|e| {
                tracing::error!(id_str = %id_str, error = %e, "corrupt letter audience id in DB");
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
            name: row.get(1)?,
            system_prompt: row.get(2)?,
            user_template: row.get(3)?,
            is_builtin: is_builtin_int != 0,
            created_at: crate::parse_db_timestamp(
                5,
                &created_str,
                "letter_audiences.created_at"
            )?,
            updated_at: crate::parse_db_timestamp(
                6,
                &updated_str,
                "letter_audiences.updated_at"
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Connection, Database, DbError, LetterAudiencesRepo};
    use medical_core::types::letter_audience::LetterAudience;
    use uuid::Uuid;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        crate::migrations::MigrationEngine::migrate(&conn).expect("migrate");
        conn
    }

    #[test]
    fn list_all_includes_builtins() {
        let conn = fresh_conn();
        let audiences = LetterAudiencesRepo::list_all(&conn).expect("list_all");
        assert_eq!(
            audiences.len(),
            6,
            "fresh DB should have 6 built-in audiences"
        );
        // Built-ins should come first (is_builtin DESC)
        assert!(audiences.iter().all(|a| a.is_builtin));
    }

    #[test]
    fn upsert_and_get() {
        let conn = fresh_conn();
        let audience = LetterAudience::new(
            "Test Audience",
            "Test system prompt",
            Some("Test template".to_string()),
        );
        let id = audience.id;

        LetterAudiencesRepo::upsert(&conn, &audience).expect("upsert");
        let fetched = LetterAudiencesRepo::get_by_id(&conn, &id).expect("get_by_id");

        assert_eq!(fetched.id, id);
        assert_eq!(fetched.name, "Test Audience");
        assert_eq!(fetched.system_prompt, "Test system prompt");
        assert_eq!(fetched.user_template, Some("Test template".to_string()));
        assert!(!fetched.is_builtin);
    }

    #[test]
    fn delete_builtin_fails() {
        let conn = fresh_conn();
        // Use the Patient builtin's UUID
        let builtin_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();

        let result = LetterAudiencesRepo::delete(&conn, &builtin_id);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::DbError::Constraint(_)),
            "expected Constraint error, got: {err:?}"
        );
    }

    #[test]
    fn delete_custom_succeeds() {
        let conn = fresh_conn();
        let audience = LetterAudience::new("To Delete", "Delete me", None);
        let id = audience.id;

        LetterAudiencesRepo::upsert(&conn, &audience).expect("upsert");
        LetterAudiencesRepo::delete(&conn, &id).expect("delete");

        let result = LetterAudiencesRepo::get_by_id(&conn, &id);
        assert!(result.is_err(), "audience should no longer exist");
    }

    #[test]
    fn get_by_id_not_found() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        let result = LetterAudiencesRepo::get_by_id(&conn, &Uuid::new_v4());
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[test]
    fn delete_not_found() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        let result = LetterAudiencesRepo::delete(&conn, &Uuid::new_v4());
        assert!(matches!(result, Err(DbError::NotFound(_))));
    }

    #[test]
    fn upsert_updates_existing() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        let mut audience =
            LetterAudience::new("Original".to_string(), "Original prompt".to_string(), None);
        LetterAudiencesRepo::upsert(&conn, &audience).unwrap();

        audience.name = "Updated".to_string();
        audience.system_prompt = "Updated prompt".to_string();
        audience.user_template = Some("Template".to_string());
        LetterAudiencesRepo::upsert(&conn, &audience).unwrap();

        let fetched = LetterAudiencesRepo::get_by_id(&conn, &audience.id).unwrap();
        assert_eq!(fetched.name, "Updated");
        assert_eq!(fetched.system_prompt, "Updated prompt");
        assert_eq!(fetched.user_template, Some("Template".to_string()));
    }
}
