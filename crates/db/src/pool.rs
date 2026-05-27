//! SQLite connection pool with WAL mode and sensible defaults.

use std::path::Path;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use crate::{DbError, DbResult};

/// Shared connection pool type.
///
/// Wraps `r2d2::Pool<SqliteConnectionManager>`. File-backed pools use
/// `max_size=8`; in-memory pools use `max_size=1`.
pub type DbPool = Pool<SqliteConnectionManager>;

/// A pooled connection checked out from [`DbPool`].
///
/// Automatically returns to the pool when dropped. Bound to the thread that
/// checked it out.
pub type PooledConnection = r2d2::PooledConnection<SqliteConnectionManager>;

fn apply_pragmas(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;",
    )
}

/// Create a file-backed connection pool with WAL mode enabled.
///
/// When `db_key` is `Some`, the pool applies `PRAGMA key="x'<hex>'"` on every
/// new connection before the standard pragmas, so the file is opened as a
/// SQLCipher-encrypted database.
///
/// Standard pragmas applied to every connection:
/// - `journal_mode=WAL`
/// - `synchronous=NORMAL`
/// - `foreign_keys=ON`
/// - `busy_timeout=5000`
///
/// # Errors
///
/// Returns [`DbError::Pool`] if the pool cannot be built.
pub fn create_pool(db_path: &Path, db_key: Option<[u8; 32]>) -> DbResult<DbPool> {
    let manager = SqliteConnectionManager::file(db_path)
        .with_init(move |conn| apply_init(conn, db_key.as_ref()));
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(DbError::Pool)?;
    Ok(pool)
}

/// Run on every fresh connection: apply the encryption key first (if any),
/// then the standard pragmas.
fn apply_init(conn: &Connection, db_key: Option<&[u8; 32]>) -> rusqlite::Result<()> {
    if let Some(key) = db_key {
        crate::encryption::apply_pragma_key(conn, key)?;
    }
    apply_pragmas(conn)
}

/// Create an in-memory connection pool (useful for tests).
///
/// Uses `max_size=1` because each SQLite in-memory connection is a separate
/// database. Standard pragmas (WAL, foreign keys, busy timeout) are still
/// applied.
///
/// # Errors
///
/// Returns [`DbError::Pool`] if the pool cannot be built.
pub fn create_memory_pool() -> DbResult<DbPool> {
    let manager = SqliteConnectionManager::memory().with_init(|conn| apply_pragmas(conn));
    let pool = Pool::builder()
        .max_size(1)
        .build(manager)
        .map_err(DbError::Pool)?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn memory_pool_creates_and_connects() {
        let pool = create_memory_pool().expect("memory pool should be created");
        let conn = pool.get().expect("should get connection");
        let result: i64 = conn
            .query_row("SELECT 1", [], |r| r.get(0))
            .expect("simple query should work");
        assert_eq!(result, 1);
    }

    #[test]
    fn foreign_keys_enabled() {
        let pool = create_memory_pool().expect("pool");
        let conn = pool.get().expect("conn");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .expect("pragma query");
        assert_eq!(fk, 1, "foreign_keys must be ON");
    }

    #[test]
    fn file_pool_wal_mode() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let pool = create_pool(&db_path, None).expect("file pool");
        let conn = pool.get().expect("conn");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("journal_mode pragma");
        assert_eq!(mode, "wal", "journal mode must be WAL");
    }
}
