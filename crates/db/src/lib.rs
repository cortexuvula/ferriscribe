pub mod pool;
pub mod encryption;
pub mod migrations;
pub mod recordings;
pub mod processing_queue;
pub mod recipients;
pub mod settings;
pub mod audit;
pub mod search;
pub mod vocabulary;
pub mod vectors;
pub mod generations;
pub use generations::{Generation, GenerationInsert, GenerationsRepo};
// CozoDB-backed knowledge graph. Gated behind the `graph` feature because cozo
// uses the Sled storage engine and brings in a non-trivial dependency tree.
// Enable with: cargo build -p medical-db --features graph
#[cfg(feature = "graph")]
pub mod graph;

use std::path::Path;

use thiserror::Error;

pub use pool::{DbPool, PooledConnection};
/// Re-export rusqlite's Connection so downstream crates can reference the
/// type (e.g. for helper signatures) without taking a direct dep on rusqlite.
pub use rusqlite::Connection;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Pool error: {0}")]
    Pool(#[from] r2d2::Error),
    #[error("Migration error: {0}")]
    Migration(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Constraint violation: {0}")]
    Constraint(String),
    #[error("Graph error: {0}")]
    Graph(String),
    #[error("{0}")]
    Other(String),
}

pub type DbResult<T> = Result<T, DbError>;

// ---------------------------------------------------------------------------
// Database facade
// ---------------------------------------------------------------------------

/// Top-level handle that owns the connection pool and exposes a convenience
/// API for the rest of the application.
pub struct Database {
    pool: DbPool,
}

impl Database {
    /// Open (or create) a file-backed database, running all pending migrations.
    pub fn open(db_path: &Path, db_key: Option<[u8; 32]>) -> DbResult<Self> {
        let pool = pool::create_pool(db_path, db_key)?;
        {
            let conn = pool.get().map_err(DbError::Pool)?;
            migrations::MigrationEngine::migrate(&conn)?;
        }
        Ok(Self { pool })
    }

    /// Open an in-memory database and run all migrations.  Primarily useful in
    /// tests and for ephemeral workloads.
    pub fn open_in_memory() -> DbResult<Self> {
        let pool = pool::create_memory_pool()?;
        {
            let conn = pool.get().map_err(DbError::Pool)?;
            migrations::MigrationEngine::migrate(&conn)?;
        }
        Ok(Self { pool })
    }

    /// Check out a pooled connection.
    pub fn conn(&self) -> DbResult<PooledConnection> {
        self.pool.get().map_err(DbError::Pool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn opens_in_memory() {
        let db = Database::open_in_memory().expect("open in-memory");
        let conn = db.conn().expect("conn");
        let v: i64 = conn
            .query_row("SELECT 1", [], |r| r.get(0))
            .expect("query");
        assert_eq!(v, 1);
    }

    #[test]
    fn opens_file() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("app.db");
        let db = Database::open(&db_path, None).expect("open file db");
        let conn = db.conn().expect("conn");
        // Migrations should have run — check that the recordings table exists.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='recordings'",
                [],
                |r| r.get(0),
            )
            .expect("query");
        assert_eq!(count, 1);
    }

    #[test]
    fn full_workflow() {
        use crate::audit::AuditRepo;
        use crate::recordings::RecordingsRepo;
        use crate::settings::SettingsRepo;
        use medical_core::types::recording::Recording;

        let db = Database::open_in_memory().expect("db");
        let conn = db.conn().expect("conn");

        // Insert a recording
        let rec = Recording::new("workflow.wav", PathBuf::from("/audio/workflow.wav"));
        RecordingsRepo::insert(&conn, &rec).expect("insert recording");

        // Save a setting
        SettingsRepo::set(&conn, "test_key", "test_value").expect("set setting");
        let val = SettingsRepo::get(&conn, "test_key")
            .expect("get setting")
            .expect("value present");
        assert_eq!(val, "test_value");

        // Write an audit entry
        let id = AuditRepo::append(&conn, "insert", "system", "recording", None)
            .expect("audit append");
        assert!(id > 0);

        // Verify everything is queryable
        assert_eq!(RecordingsRepo::count(&conn).expect("count"), 1);
        assert_eq!(AuditRepo::count(&conn).expect("count"), 1);
    }
}
