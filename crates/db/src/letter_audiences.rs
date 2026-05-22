use chrono::{DateTime, Utc};
use rusqlite::Connection;
use uuid::Uuid;

use medical_core::types::letter_audience::LetterAudience;

use crate::{DbError, DbResult};

pub struct LetterAudiencesRepo;

impl LetterAudiencesRepo {
    pub fn list_all(conn: &Connection) -> DbResult<Vec<LetterAudience>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, system_prompt, user_template, is_builtin, created_at, updated_at
             FROM letter_audiences
             ORDER BY is_builtin DESC, name"
        )?;
        let rows = stmt.query_map([], Self::row_to_audience)?;
        let mut audiences = Vec::new();
        for row in rows {
            audiences.push(row?);
        }
        Ok(audiences)
    }

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

    pub fn delete(conn: &Connection, id: &Uuid) -> DbResult<()> {
        // Check if the audience exists and whether it's built-in
        let is_builtin: Result<i32, _> = conn.query_row(
            "SELECT is_builtin FROM letter_audiences WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        );

        match is_builtin {
            Ok(1) => Err(DbError::Constraint(
                "cannot delete built-in letter audience".to_string(),
            )),
            Ok(0) => {
                conn.execute(
                    "DELETE FROM letter_audiences WHERE id = ?1",
                    [id.to_string()],
                )?;
                Ok(())
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(DbError::NotFound(format!("letter audience {id}")))
            }
            Err(other) => Err(DbError::Sqlite(other)),
            Ok(_) => unreachable!("is_builtin should only be 0 or 1"),
        }
    }

    fn row_to_audience(row: &rusqlite::Row<'_>) -> rusqlite::Result<LetterAudience> {
        let id_str: String = row.get(0)?;
        let is_builtin_int: i32 = row.get(4)?;
        let created_str: String = row.get(5)?;
        let updated_str: String = row.get(6)?;

        Ok(LetterAudience {
            id: Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::nil()),
            name: row.get(1)?,
            system_prompt: row.get(2)?,
            user_template: row.get(3)?,
            is_builtin: is_builtin_int != 0,
            created_at: DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&updated_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Connection, LetterAudiencesRepo};
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
        let audience = LetterAudience::new(
            "To Delete",
            "Delete me",
            None,
        );
        let id = audience.id;

        LetterAudiencesRepo::upsert(&conn, &audience).expect("upsert");
        LetterAudiencesRepo::delete(&conn, &id).expect("delete");

        let result = LetterAudiencesRepo::get_by_id(&conn, &id);
        assert!(result.is_err(), "audience should no longer exist");
    }
}
