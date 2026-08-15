//! SQLite database layer for the FerriScribe medical transcription app.
//!
//! This crate owns all persistent state: consultation recordings, application
//! settings, vocabulary rules, vector embeddings for RAG, a SQLite-backed
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
//! # Thread safety
//!
//! `DbPool` is `Send + Sync`. Each `PooledConnection` is bound to the thread
//! that checked it out. SQLite WAL mode allows concurrent readers with one
//! writer; `busy_timeout=5000` mitigates transient write contention.

pub mod audit;
pub mod condition_chips;
pub mod content_sync;
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
pub use condition_chips::ConditionChipsRepo;
pub use content_sync::{
    ContentSyncRepo, FieldRevision, MergeConflict, MergeResult, SyncCursor, SyncFieldValue,
    SyncRecording,
};
pub use user_dictionary::UserDictionaryRepo;

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

/// Parse a timestamp column that may be stored in either of two legitimate
/// formats: RFC 3339 (e.g. `2026-05-22T00:00:00Z`, used by rows written from
/// Rust via `to_rfc3339()`) or SQLite's native `datetime('now')` format
/// (`2026-05-22 00:00:00`, used by columns with `DEFAULT (datetime('now'))`
/// when no explicit value is supplied). Both are valid stored data; only
/// genuinely corrupt strings surface as a `FromSqlConversionFailure` error
/// instead of silently falling back to the current time.
pub(crate) fn parse_db_timestamp(
    col_index: usize,
    ts_str: &str,
    column_label: &str,
) -> rusqlite::Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    // Fast path: RFC 3339.
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
        return Ok(dt.with_timezone(&Utc));
    }
    // SQLite `datetime('now')` format: "YYYY-MM-DD HH:MM:SS" (UTC).
    match NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S") {
        Ok(ndt) => Ok(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc)),
        Err(e) => {
            tracing::error!(
                ts_str = %ts_str,
                column = %column_label,
                error = %e,
                "corrupt timestamp in DB"
            );
            Err(rusqlite::Error::FromSqlConversionFailure(
                col_index,
                rusqlite::types::Type::Text,
                Box::new(e),
            ))
        }
    }
}

/// Convert a [`DbError`] into an [`AppError`], preserving the source chain.
///
/// The `DbError` is stored as the `source` of the `AppError::Database`
/// variant so that `Error::source()` returns the original typed error
/// (e.g. `rusqlite::Error`), enabling structured error inspection at
/// crate boundaries.
impl From<DbError> for medical_core::error::AppError {
    fn from(e: DbError) -> Self {
        medical_core::error::AppError::database_with_source(e.to_string(), e)
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
