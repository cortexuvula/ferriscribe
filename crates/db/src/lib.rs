//! SQLite database layer for the FerriScribe medical transcription app.
//!
//! This crate owns all persistent state: consultation recordings, application
//! settings, vocabulary rules, vector embeddings for RAG, a CozoDB-backed
//! medical knowledge graph, processing queues, generation history, letter
//! audiences, a user dictionary, and an append-only audit log.
//!
//! # Architecture
//!
//! The top-level entry point is [`Database`], which wraps an `r2d2` connection
//! pool and runs all pending migrations on open. Individual domain areas are
//! exposed as stateless repository structs (e.g. [`recordings::RecordingsRepo`],
//! [`settings::SettingsRepo`]) whose methods take a `&Connection`.
//!
//! # Feature flags
//!
//! - **`graph`** -- enables the `graph` module (CozoDB-backed knowledge
//!   graph). Gated because CozoDB pulls in the Sled storage engine.
//!
//! # Thread safety
//!
//! `DbPool` is `Send + Sync`. Each `PooledConnection` is bound to the thread
//! that checked it out. SQLite WAL mode allows concurrent readers with one
//! writer; `busy_timeout=5000` mitigates transient write contention.

pub mod audit;
pub mod condition_chips;
pub mod encryption;
pub mod letter_audiences;
pub mod migrations;
pub mod pool;
pub mod processing_queue;
pub mod recipients;
pub mod recordings;
pub mod search;
pub mod settings;
pub mod vocabulary;
pub use letter_audiences::LetterAudiencesRepo;
pub mod generations;
pub mod vectors;
pub use generations::{Generation, GenerationInsert, GenerationsRepo};
pub mod user_dictionary;
pub use user_dictionary::UserDictionaryRepo;
pub use condition_chips::ConditionChipsRepo;

use std::path::Path;

use thiserror::Error;

pub use pool::{DbPool, PooledConnection};
/// Re-export rusqlite's Connection so downstream crates can reference the
/// type (e.g. for helper signatures) without taking a direct dep on rusqlite.
pub use rusqlite::Connection;

/// Errors produced by database operations.
///
/// Every repository method returns [`DbResult<T>`] which is an alias for
/// `Result<T, DbError>`.
#[derive(Error, Debug)]
pub enum DbError {
    /// Wrapped `rusqlite::Error` from any SQLite operation.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Error checking out a connection from the `r2d2` pool.
    #[error("Pool error: {0}")]
    Pool(#[from] r2d2::Error),
    /// A schema migration failed.
    #[error("Migration error: {0}")]
    Migration(String),
    /// The requested row was not found.
    #[error("Not found: {0}")]
    NotFound(String),
    /// A database constraint was violated (e.g. deleting a built-in row).
    #[error("Constraint violation: {0}")]
    Constraint(String),
    /// An error from the CozoDB-backed knowledge graph.
    #[error("Graph error: {0}")]
    Graph(String),
    /// A string could not be parsed as a valid UUID.
    #[error("UUID parse error in {1}: {0}")]
    UuidParse(String, String),
    /// An I/O error from the filesystem (file copy, migration write, backup).
    /// Added so these failures get a typed variant instead of being stringified
    /// into the opaque `Other(String)` catch-all.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Catch-all for other database errors.
    #[error("{0}")]
    Other(String),
}

/// Convenience result type for database operations.
pub type DbResult<T> = Result<T, DbError>;

/// Convert a [`DbError`] into an [`AppError`].
///
/// `DbError`'s `Display` already includes its discriminant text (e.g.
/// "Not found: ...", "Constraint violation: ..."), so mapping to
/// `AppError::Database(e.to_string())` preserves the structured error info
/// in the message string. This lets call sites use `?` instead of the
/// ~90 `.map_err(AppError::from)` closures that
/// previously existed across `src-tauri/src/commands/`.
impl From<DbError> for medical_core::error::AppError {
    fn from(e: DbError) -> Self {
        medical_core::error::AppError::Database(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Database facade
// ---------------------------------------------------------------------------

/// Top-level handle that owns the connection pool and exposes a convenience
/// API for the rest of the application.
///
/// Create one instance at app startup and share it across threads. All
/// repository methods accept a `&Connection` obtained via [`Database::conn`].
pub struct Database {
    pool: DbPool,
}

impl Database {
    /// Open (or create) a file-backed database, running all pending migrations.
    ///
    /// When `db_key` is `Some`, the file is opened as a SQLCipher-encrypted
    /// database using the supplied 32-byte key.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Pool`] if the connection pool cannot be created, or
    /// [`DbError::Migration`] / [`DbError::Sqlite`] if any migration fails.
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
    ///
    /// The pool has `max_size=1` because each SQLite in-memory connection is a
    /// separate database.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Pool`] or [`DbError::Migration`] on failure.
    pub fn open_in_memory() -> DbResult<Self> {
        let pool = pool::create_memory_pool()?;
        {
            let conn = pool.get().map_err(DbError::Pool)?;
            migrations::MigrationEngine::migrate(&conn)?;
        }
        Ok(Self { pool })
    }

    /// Check out a pooled connection.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Pool`] if the pool is exhausted or a connection
    /// cannot be established.
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
        let v: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("query");
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
        let id =
            AuditRepo::append(&conn, "insert", "system", "recording", None).expect("audit append");
        assert!(id > 0);

        // Verify everything is queryable
        assert_eq!(RecordingsRepo::count(&conn).expect("count"), 1);
        assert_eq!(AuditRepo::count(&conn).expect("count"), 1);
    }
}
