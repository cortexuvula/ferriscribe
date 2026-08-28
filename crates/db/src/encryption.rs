//! Database encryption helpers for SQLCipher.
//!
//! The pool's `with_init` callback uses `apply_pragma_key` (when a key is
//! supplied) before any other PRAGMA, since SQLCipher requires the key to be
//! the very first statement on a fresh connection.

use rusqlite::Connection;

use crate::DbResult;

/// Apply `PRAGMA key="x'<hex>'"` so SQLCipher can decrypt the DB.
///
/// Must run before any other statement on the connection. The key is
/// encoded as a hex blob literal (`x'...'`) so SQLCipher interprets it
/// as 32 raw bytes.
///
/// # Errors
///
/// Returns a `rusqlite::Error` if the PRAGMA fails.
pub fn apply_pragma_key(conn: &Connection, key: &[u8; 32]) -> rusqlite::Result<()> {
    let hex_key = hex::encode(key);
    // Use the x'...' blob literal form so the key is interpreted as 32 bytes
    // rather than as the ASCII string of hex digits.
    conn.execute_batch(&format!("PRAGMA key=\"x'{hex_key}'\";"))
}

/// Verify the encryption key by running a trivial read.
///
/// Runs `SELECT count(*) FROM sqlite_master` which forces SQLCipher to
/// decrypt page 1.
///
/// # Errors
///
/// Returns `Err` if SQLCipher rejects the key (wrong key, file not
/// encrypted, or file corrupt).
pub fn verify_key(conn: &Connection) -> rusqlite::Result<()> {
    // SELECT count(*) FROM sqlite_master is the canonical key-verification
    // probe — it forces SQLCipher to decrypt page 1.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
}

/// Detect whether the given DB file is plaintext (SQLite header) or
/// encrypted (SQLCipher header).
///
/// Plaintext returns `Ok(true)`; encrypted returns `Ok(false)`. A missing
/// file also returns `Ok(false)` (the caller decides what to do).
///
/// Detection works by checking for the 16-byte SQLite magic string
/// `"SQLite format 3\0"`.
pub fn is_plaintext_db(db_path: &std::path::Path) -> DbResult<bool> {
    use std::io::Read;
    if !db_path.exists() {
        return Ok(false); // treat missing as not-plaintext (caller decides what to do)
    }
    let mut f = std::fs::File::open(db_path)
        .map_err(|e| crate::DbError::Other(format!("open {db_path:?}: {e}")))?;
    let mut header = [0u8; 16];
    let n = f
        .read(&mut header)
        .map_err(|e| crate::DbError::Other(format!("read {db_path:?}: {e}")))?;
    if n < 16 {
        return Ok(false);
    }
    // SQLite plaintext files start with the magic string "SQLite format 3\0"
    // (16 bytes). SQLCipher files start with random ciphertext.
    Ok(&header == b"SQLite format 3\0")
}

use std::path::{Path, PathBuf};

/// Outcome of a successful [`migrate_plaintext_to_encrypted`] call.
#[derive(Debug)]
pub struct MigrationOutcome {
    /// Final path of the encrypted database (same as the input path on success).
    pub encrypted_path: PathBuf,
    /// Whether the backup file was deleted (true on full success).
    pub backup_deleted: bool,
}

/// Migrate a plaintext SQLite DB to an encrypted SQLCipher DB.
///
/// Sequence:
/// 1. Backup `db_path` to `<db_path>.pre-encryption.bak`.
/// 2. Open empty encrypted DB at `<db_path>.encrypting`.
/// 3. Use `sqlcipher_export()` to copy all data from plaintext.
/// 4. Verify row counts table-by-table.
/// 5. Atomic rename: encrypting -> original; delete backup.
///
/// On any failure after step 1, the original is restored from the backup
/// and the error is returned.
///
/// # Errors
///
/// Returns [`DbError::Other`](crate::DbError::Other) on any I/O or
/// SQLCipher failure. The original file is restored from backup on failure.
pub fn migrate_plaintext_to_encrypted(
    db_path: &Path,
    db_key: &[u8; 32],
) -> DbResult<MigrationOutcome> {
    let backup_path = backup_path_for(db_path);
    let encrypting_path = encrypting_path_for(db_path);

    // Step 0: fold any WAL frames into the main file and drop the
    // sidecars. A crashed plaintext session can leave committed
    // transactions living only in `<db>-wal`; the plain file copy below
    // would silently miss them. This is also why the sidecars must not
    // survive next to the file once it becomes a SQLCipher DB: SQLite
    // would try to replay plaintext frames against encrypted pages.
    checkpoint_and_clear_wal(db_path)?;

    // Step 1: backup (must succeed before we touch anything else).
    std::fs::copy(db_path, &backup_path).map_err(|e| {
        crate::DbError::Io(std::io::Error::new(
            e.kind(),
            format!("backup copy failed: {e}"),
        ))
    })?;

    // Wrap the rest in a closure so we can restore on failure.
    let result: DbResult<()> = (|| {
        // Step 2: open empty encrypted DB
        if encrypting_path.exists() {
            std::fs::remove_file(&encrypting_path).ok();
        }
        let enc = Connection::open(&encrypting_path)
            .map_err(|e| crate::DbError::Other(format!("open encrypting: {e}")))?;
        apply_pragma_key(&enc, db_key)
            .map_err(|e| crate::DbError::Other(format!("pragma key on new encrypted DB: {e}")))?;

        // Step 3: ATTACH plaintext, sqlcipher_export, DETACH.
        // The plaintext is opened with KEY '' (empty) which tells SQLCipher
        // to treat it as a non-encrypted SQLite database.
        let plaintext_str = db_path.to_string_lossy().replace('\'', "''");
        enc.execute_batch(&format!(
            "ATTACH DATABASE '{plaintext_str}' AS plaintext KEY '';\n\
             SELECT sqlcipher_export('main', 'plaintext');\n\
             DETACH DATABASE plaintext;"
        ))
        .map_err(|e| crate::DbError::Other(format!("sqlcipher_export: {e}")))?;
        drop(enc);

        // Step 4: verify by re-opening and comparing row counts.
        verify_row_counts(db_path, &encrypting_path, db_key)?;

        // Step 5: atomic swap (replace original with encrypted). fsync first so
        // that a crash between the rename and the backup deletion can't leave a
        // truncated encrypted DB (ext4 delayed allocation can otherwise lose
        // unwritten pages on power loss).
        std::fs::File::open(&encrypting_path)
            .and_then(|f| f.sync_all())
            .map_err(|e| {
                crate::DbError::Io(std::io::Error::new(
                    e.kind(),
                    format!("fsync encrypted DB: {e}"),
                ))
            })?;
        std::fs::rename(&encrypting_path, db_path).map_err(|e| {
            crate::DbError::Io(std::io::Error::new(e.kind(), format!("rename failed: {e}")))
        })?;
        // verify_row_counts re-opened the plaintext DB, which can recreate
        // sidecars — clear them so nothing plaintext survives next to the
        // now-encrypted file.
        remove_wal_sidecars(db_path);
        Ok(())
    })();

    match result {
        Ok(()) => {
            // On success delete the backup. The DB is already encrypted at
            // this point, so a failed delete must NOT fail the migration
            // (the Err path would restore the plaintext backup over the
            // encrypted file) — but the outcome must say a plaintext copy
            // is still on disk so the caller can surface it.
            let backup_deleted = match std::fs::remove_file(&backup_path) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "encryption succeeded but the plaintext backup could not be deleted — it remains on disk"
                    );
                    false
                }
            };
            Ok(MigrationOutcome {
                encrypted_path: db_path.to_path_buf(),
                backup_deleted,
            })
        }
        Err(e) => {
            // Restore from backup; clean up partial encrypted file.
            let _ = std::fs::rename(&backup_path, db_path);
            let _ = std::fs::remove_file(&encrypting_path);
            // The stale sidecars belong to the pre-migration state that no
            // longer matches the restored file's history — drop them too.
            remove_wal_sidecars(db_path);
            Err(e)
        }
    }
}

/// Checkpoint the DB's WAL into the main file, then remove `-wal`/`-shm`.
///
/// Harmless on non-WAL databases (`PRAGMA wal_checkpoint` no-ops, the
/// sidecars don't exist). Best-effort on the removals — after a TRUNCATE
/// checkpoint the WAL is zero-length, so failing to delete it loses
/// nothing.
fn checkpoint_and_clear_wal(db_path: &Path) -> DbResult<()> {
    let conn = Connection::open(db_path)
        .map_err(|e| crate::DbError::Other(format!("wal checkpoint open: {e}")))?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| crate::DbError::Other(format!("wal checkpoint: {e}")))?;
    drop(conn);
    remove_wal_sidecars(db_path);
    Ok(())
}

fn remove_wal_sidecars(db_path: &Path) {
    for sidecar in wal_sidecar_paths(db_path) {
        let _ = std::fs::remove_file(sidecar);
    }
}

fn wal_sidecar_paths(db_path: &Path) -> [std::path::PathBuf; 2] {
    let mut wal = db_path.as_os_str().to_os_string();
    wal.push("-wal");
    let mut shm = db_path.as_os_str().to_os_string();
    shm.push("-shm");
    [std::path::PathBuf::from(wal), std::path::PathBuf::from(shm)]
}

/// Compare row counts table-by-table between the plaintext and encrypted DBs.
/// Returns Err on any mismatch. Used internally by `migrate_plaintext_to_encrypted`
/// but exposed for diagnostic use too.
fn verify_row_counts(
    plaintext_path: &Path,
    encrypted_path: &Path,
    db_key: &[u8; 32],
) -> DbResult<()> {
    let plaintext = Connection::open(plaintext_path)
        .map_err(|e| crate::DbError::Other(format!("verify open plaintext: {e}")))?;
    let encrypted = Connection::open(encrypted_path)
        .map_err(|e| crate::DbError::Other(format!("verify open encrypted: {e}")))?;
    apply_pragma_key(&encrypted, db_key)
        .map_err(|e| crate::DbError::Other(format!("verify pragma key: {e}")))?;

    let tables = list_user_tables(&plaintext)?;
    for table in tables {
        let plaintext_count: i64 = plaintext
            .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .map_err(|e| crate::DbError::Other(format!("count plaintext.{table}: {e}")))?;
        let encrypted_count: i64 = encrypted
            .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .map_err(|e| crate::DbError::Other(format!("count encrypted.{table}: {e}")))?;
        if plaintext_count != encrypted_count {
            return Err(crate::DbError::Other(format!(
                "row count mismatch in table {table}: plaintext={plaintext_count}, encrypted={encrypted_count}"
            )));
        }
    }
    Ok(())
}

fn list_user_tables(conn: &Connection) -> DbResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|e| crate::DbError::Other(format!("list tables: {e}")))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| crate::DbError::Other(format!("list tables iter: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| crate::DbError::Other(format!("list tables collect: {e}")))?;
    // Defensive: table names are interpolated into a double-quoted identifier
    // in verify_row_counts. Reject any name that isn't a plain SQL identifier
    // (starts with letter/underscore, alphanumeric+underscore only) so a
    // hostile name containing a double-quote or backtick can't break out of
    // the quoting. Real tables created by this app always match this pattern.
    let valid = names
        .into_iter()
        .filter(|n| is_safe_identifier(n))
        .collect();
    Ok(valid)
}

/// True iff `name` is a plain SQL identifier: starts with ASCII letter or
/// underscore, followed by ASCII letters/digits/underscores. Used to guard
/// identifier interpolation against injection.
fn is_safe_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn backup_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".pre-encryption.bak");
    PathBuf::from(s)
}

fn encrypting_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".encrypting");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_pragma_key_does_not_error_on_in_memory_db() {
        // With SQLCipher bundled, applying PRAGMA key on an in-memory DB
        // succeeds and creates an encrypted-in-memory connection. We're
        // checking that the call returns Ok, not the cipher behavior.
        let conn = Connection::open_in_memory().unwrap();
        let key = [0xABu8; 32];
        apply_pragma_key(&conn, &key).expect("pragma key");
    }

    #[test]
    fn is_plaintext_db_returns_false_for_missing_file() {
        let path = std::path::PathBuf::from("/nonexistent/path.db");
        let result = is_plaintext_db(&path).unwrap();
        assert!(!result);
    }

    #[test]
    fn is_plaintext_db_recognizes_sqlite_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t(x);").unwrap();
        drop(conn);
        assert!(is_plaintext_db(&path).unwrap());
    }

    #[test]
    fn migration_folds_wal_frames_and_clears_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.db");

        // Simulate a crashed plaintext session: WAL mode, a committed
        // transaction, and the connection still open so SQLite never got
        // its clean-close checkpoint. The committed row lives ONLY in
        // <db>-wal — a plain fs::copy of the main file would miss it.
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (42);")
            .unwrap();
        assert!(
            path.with_file_name("plain.db-wal").exists(),
            "fixture must leave committed frames in the WAL"
        );

        let key = [0xCDu8; 32];
        let outcome = migrate_plaintext_to_encrypted(&path, &key).expect("migrate");
        assert!(outcome.backup_deleted);

        // No plaintext sidecars next to the encrypted DB.
        for sidecar in wal_sidecar_paths(&path) {
            assert!(!sidecar.exists(), "{} must not survive", sidecar.display());
        }

        // The WAL-committed row made it into the encrypted DB.
        let reader = Connection::open(&path).unwrap();
        apply_pragma_key(&reader, &key).unwrap();
        let x: i64 = reader
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(x, 42);
    }
}
