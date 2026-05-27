//! Migration engine for the medical-db crate.
//!
//! Migrations are identified by a monotonically-increasing version number.
//! Applied migrations are recorded in the `schema_version` table so that
//! re-running the engine is idempotent.

pub mod m001_initial;
pub mod m002_rag_tables;
pub mod m003_vocabulary;
pub mod m004_generations;
pub mod m005_user_dictionary;
pub mod m006_letter_audiences;

use rusqlite::Connection;

use crate::DbResult;

/// A single schema migration.
///
/// Each migration has a monotonically increasing `version` number, a
/// human-readable `name`, and an `up` function that applies the schema
/// changes. Migrations are applied in version order by
/// [`MigrationEngine::migrate`].
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub up: fn(&Connection) -> DbResult<()>,
}

/// Returns the complete ordered list of all known migrations.
///
/// New migrations must be appended here with the next available version
/// number. The returned slice is `'static` so it can be iterated without
/// lifetime concerns.
pub fn all_migrations() -> &'static [Migration] {
    &[
        Migration {
            version: 1,
            name: "initial_schema",
            up: m001_initial::up,
        },
        Migration {
            version: 2,
            name: "rag_tables",
            up: m002_rag_tables::up,
        },
        Migration {
            version: 3,
            name: "vocabulary",
            up: m003_vocabulary::up,
        },
        Migration {
            version: 4,
            name: "generations",
            up: m004_generations::up,
        },
        Migration {
            version: 5,
            name: "user_dictionary",
            up: m005_user_dictionary::up,
        },
        Migration {
            version: 6,
            name: "letter_audiences",
            up: m006_letter_audiences::up,
        },
    ]
}

/// Manages applying pending migrations in order.
///
/// The engine records applied migration versions in the `schema_version`
/// table, making re-runs idempotent. Call [`migrate`](Self::migrate) once
/// at application startup.
pub struct MigrationEngine;

impl MigrationEngine {
    /// Ensure the `schema_version` table exists, then apply every migration
    /// whose version is greater than the currently recorded version.
    ///
    /// Returns the number of newly applied migrations (0 if already
    /// up-to-date).
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Sqlite`](crate::DbError::Sqlite) or
    /// [`DbError::Migration`](crate::DbError::Migration) if any migration
    /// fails. Partially applied migrations are **not** rolled back -- fix
    /// the failing migration and re-run.
    pub fn migrate(conn: &Connection) -> DbResult<u32> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version     INTEGER NOT NULL,
                name        TEXT NOT NULL,
                applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;

        let current = Self::current_version(conn)?;
        let mut applied = 0u32;

        for migration in all_migrations() {
            if migration.version > current {
                (migration.up)(conn)?;
                conn.execute(
                    "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
                    [&migration.version.to_string(), migration.name],
                )?;
                applied += 1;
            }
        }

        Ok(applied)
    }

    /// Returns the highest migration version that has been successfully
    /// applied, or `0` if the database is empty / the table has no rows.
    pub fn current_version(conn: &Connection) -> DbResult<u32> {
        // If the schema_version table doesn't exist yet we return 0.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);

        if !exists {
            return Ok(0);
        }

        let version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_db_version_zero() {
        let conn = fresh();
        let v = MigrationEngine::current_version(&conn).expect("version");
        assert_eq!(v, 0);
    }

    #[test]
    fn migrate_applies_all() {
        let conn = fresh();
        let applied = MigrationEngine::migrate(&conn).expect("migrate");
        assert_eq!(applied, all_migrations().len() as u32);
    }

    #[test]
    fn idempotent() {
        let conn = fresh();
        MigrationEngine::migrate(&conn).expect("first migrate");
        let second = MigrationEngine::migrate(&conn).expect("second migrate");
        assert_eq!(second, 0, "no migrations should be applied the second time");
    }

    #[test]
    fn tracks_in_schema_version() {
        let conn = fresh();
        MigrationEngine::migrate(&conn).expect("migrate");
        let v = MigrationEngine::current_version(&conn).expect("version");
        assert_eq!(v, all_migrations().last().unwrap().version);
    }
}
