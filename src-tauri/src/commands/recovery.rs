//! Tauri commands for the database recovery flow.
//!
//! These commands deliberately do NOT depend on `AppState` — when the boot
//! flow returns `InitError::DatabaseRecoveryNeeded`, no `AppState` is
//! managed, so the recovery commands operate directly on filesystem paths
//! and the keychain. `RecoveryState` is always managed (with `Some(reason)`
//! during recovery and `None` on normal boot) so the frontend can query the
//! recovery reason on mount instead of subscribing to a timing-race event.

use std::path::PathBuf;
use std::sync::Arc;

use medical_core::error::{AppError, AppResult};

use crate::state::{FatalErrorState, RecoveryState};

/// Return the current recovery reason. The frontend invokes this on mount;
/// if `Some`, it renders the recovery dialog.
#[tauri::command]
pub fn get_database_recovery_state(state: tauri::State<'_, Arc<RecoveryState>>) -> Option<String> {
    state.get()
}

/// Return the fatal initialization error, if any. The frontend invokes this on
/// mount alongside `get_database_recovery_state`; if `Some`, it renders the
/// fatal-error dialog. This replaces the old `panic!` on `InitError::Other`,
/// which under `panic = "abort"` (release) was a silent hard exit.
#[tauri::command]
pub fn get_fatal_error(state: tauri::State<'_, Arc<FatalErrorState>>) -> Option<String> {
    state.get()
}

/// Restore from a user-picked plaintext backup. Copies the file into place,
/// generates a new keychain key, and runs the encryption migration. The
/// frontend should reload the window after this returns `Ok`.
///
/// Everything runs on the blocking pool: the backup copy can be hundreds of
/// MB, the keychain round trips go through OS IPC, and the encryption
/// migration rewrites the whole DB — none of that belongs on the runtime.
#[tauri::command]
pub async fn recover_database_from_path(backup_path: String) -> AppResult<()> {
    let backup = PathBuf::from(&backup_path);
    if !backup.exists() {
        return Err(AppError::Other(format!(
            "backup file not found: {backup_path}"
        )));
    }

    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let data_dir = data_dir()?;
        recover_database_from_path_at(&backup, &data_dir)
    })
    .await
    .map_err(|e| AppError::Other(format!("recovery task join: {e}")))?
}

/// Testable core of [`recover_database_from_path`] with the data dir
/// injected (the command resolves the real one; tests pass a tempdir so
/// they never touch the developer's actual DB or keychain state beyond the
/// keychain test mock).
///
/// Ordering contract — the ORIGINAL is never destroyed before the
/// replacement is safely in place:
///
/// 1. copy backup → `medical.db.recovery-tmp` in the data dir;
/// 2. fsync the copy (`File::sync_all`) so a crash right after the swap
///    never exposes a truncated DB;
/// 3. rename the tmp over `medical.db` (atomic on one filesystem);
/// 4. remove stale `db-shm`/`db-wal` sidecars — only AFTER the successful
///    rename (before it they belong to the still-live previous DB);
/// 5. wipe the keychain key, mint a fresh one, run the encryption
///    migration.
///
/// On a copy/fsync/rename failure the tmp file is cleaned up and the error
/// returned with the existing DB untouched (the old code deleted the DB and
/// wiped the keychain BEFORE the copy — a failed copy destroyed the only
/// good copy of the data).
fn recover_database_from_path_at(
    backup: &std::path::Path,
    data_dir: &std::path::Path,
) -> AppResult<()> {
    if !medical_db::encryption::is_plaintext_db(backup)
        .map_err(|e| AppError::Other(format!("inspect backup: {e}")))?
    {
        return Err(AppError::Other(
            "Selected file is not a plaintext SQLite database. Encrypted backups \
             cannot be restored without the original keychain entry."
                .into(),
        ));
    }

    std::fs::create_dir_all(data_dir)
        .map_err(|e| AppError::Other(format!("create data dir: {e}")))?;
    let db_path = data_dir.join("medical.db");
    let tmp_path = data_dir.join("medical.db.recovery-tmp");

    // 1-2. Copy to a tmp sibling and fsync it. Any failure here cleans the
    // tmp and leaves the existing DB (and keychain) untouched.
    if let Err(e) = std::fs::copy(backup, &tmp_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AppError::Other(format!("copy backup: {e}")));
    }
    if let Err(e) = std::fs::File::open(&tmp_path).and_then(|f| f.sync_all()) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AppError::Other(format!("sync backup copy: {e}")));
    }

    // 3. Swap the replacement in.
    if let Err(e) = std::fs::rename(&tmp_path, &db_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AppError::Other(format!("install backup: {e}")));
    }

    // 4. Stale sidecars from the previous DB must not survive next to the
    // replaced file (SQLite would replay old frames against new pages).
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));

    // 5. Only NOW is the old DB gone: retire its key and migrate the
    // replacement to an encrypted DB under a fresh one.
    medical_security::keychain::wipe_db_key()
        .map_err(|e| AppError::Other(format!("clear keychain: {e}")))?;
    let key = medical_security::keychain::get_or_create_db_key()
        .map_err(|e| AppError::Other(format!("create key: {e}")))?;
    medical_db::encryption::migrate_plaintext_to_encrypted(&db_path, &key)
        .map_err(|e| AppError::Other(format!("migration: {e}")))?;

    Ok(())
}

/// Wipe the encrypted DB and the keychain entry. Frontend should reload.
///
/// Runs on the blocking pool (keychain OS IPC + file removal), matching
/// [`recover_database_from_path`].
#[tauri::command]
pub async fn recover_database_wipe() -> AppResult<()> {
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let data_dir = data_dir()?;
        let db_path = data_dir.join("medical.db");

        medical_security::keychain::wipe_db_key()
            .map_err(|e| AppError::Other(format!("wipe keychain: {e}")))?;
        if db_path.exists() {
            std::fs::remove_file(&db_path)
                .map_err(|e| AppError::Other(format!("remove db: {e}")))?;
        }
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));

        Ok(())
    })
    .await
    .map_err(|e| AppError::Other(format!("recovery task join: {e}")))?
}

/// Returns the current encryption state of the on-disk database.
/// Frontend uses this to display the Settings → Database security panel.
#[tauri::command]
pub async fn database_encryption_status() -> AppResult<serde_json::Value> {
    // Resolve data_dir the same way AppState::initialize does (NOT via
    // tauri::AppHandle::path() which uses the bundle id).
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rust-medical-assistant");
    let db_path = data_dir.join("medical.db");

    if !db_path.exists() {
        return Ok(serde_json::json!({ "state": "no-database" }));
    }

    // Header read + keychain round trip are blocking — off the async worker.
    let (plaintext, key_present) = tokio::task::spawn_blocking({
        let db_path = db_path.clone();
        move || -> AppResult<(bool, bool)> {
            let plaintext = medical_db::encryption::is_plaintext_db(&db_path)
                .map_err(|e| AppError::Other(format!("inspect: {e}")))?;
            let key_present = medical_security::keychain::get_db_key()
                .map(|opt| opt.is_some())
                .unwrap_or(false);
            Ok((plaintext, key_present))
        }
    })
    .await
    .map_err(|e| AppError::Other(format!("status task join: {e}")))??;

    Ok(serde_json::json!({
        "state": if plaintext { "plaintext" } else { "encrypted" },
        "key_present": key_present,
    }))
}

/// Resolve the same data directory used by `AppState::initialize` so the
/// recovery commands operate on the file the boot flow will read on the
/// next launch. Do NOT use `tauri::AppHandle::path().app_data_dir()` —
/// that resolves via the bundle identifier and would point at a different
/// directory.
fn data_dir() -> AppResult<PathBuf> {
    Ok(dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-medical-assistant"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture: a real plaintext SQLite file (valid header, one table) —
    /// what `is_plaintext_db` expects a user-picked backup to look like.
    fn plaintext_sqlite_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("backup.db");
        let conn = rusqlite::Connection::open(&path).expect("create plaintext db");
        conn.execute_batch("CREATE TABLE probe(x); INSERT INTO probe VALUES (42);")
            .expect("seed backup");
        drop(conn);
        assert!(
            medical_db::encryption::is_plaintext_db(&path).expect("inspect"),
            "fixture must pass the is_plaintext gate"
        );
        path
    }

    /// A failed copy must leave the existing DB untouched and no tmp file
    /// behind. The failure is driven deterministically by making the data
    /// dir read-only (the backup still passes the is_plaintext check — it
    /// only needs read permission). Unix-only: the chmod trick does not
    /// apply on Windows filesystems.
    #[cfg(unix)]
    #[test]
    fn failed_copy_leaves_existing_db_intact_and_no_tmp() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let backup = plaintext_sqlite_file(tmp.path());
        let db_path = tmp.path().join("medical.db");
        std::fs::write(&db_path, b"pre-existing encrypted-ish bytes").expect("seed live db");

        // Read-only data dir: create_dir_all no-ops on the existing dir,
        // the tmp-file copy fails with EACCES (deterministic for non-root).
        let original_mode = std::fs::metadata(tmp.path())
            .expect("data dir metadata")
            .permissions()
            .mode();
        let mut perms = std::fs::metadata(tmp.path())
            .expect("data dir metadata")
            .permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(tmp.path(), perms).expect("chmod read-only");

        let result = recover_database_from_path_at(&backup, tmp.path());

        // Restore writability so the tempdir can clean itself up.
        let mut perms = std::fs::metadata(tmp.path())
            .expect("data dir metadata")
            .permissions();
        perms.set_mode(original_mode);
        std::fs::set_permissions(tmp.path(), perms).expect("chmod restore");

        assert!(result.is_err(), "copy into a read-only dir must fail");
        assert_eq!(
            std::fs::read(&db_path).expect("original still readable"),
            b"pre-existing encrypted-ish bytes",
            "a failed copy must leave the existing DB untouched"
        );
        assert!(
            !tmp.path().join("medical.db.recovery-tmp").exists(),
            "failed copy must clean up its tmp file"
        );
    }

    /// The success path: the backup is swapped in (tmp gone), stale
    /// sidecars are removed, and the result is migrated to an encrypted DB
    /// under the keychain mock's fresh key.
    #[test]
    fn successful_recovery_swaps_db_and_migrates_to_encrypted() {
        // Keychain isolation: the wipe + get_or_create round trips route
        // through the mock, never the developer's real keychain.
        let _mock = crate::testutil::KeychainMockGuard::fixed_db_key([0x99u8; 32]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let backup = plaintext_sqlite_file(tmp.path());
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).expect("data dir");
        // A stale "previous" DB + sidecar — both replaced/cleared by the
        // recovery.
        std::fs::write(data.join("medical.db"), b"old").expect("old db");
        std::fs::write(data.join("medical.db-wal"), b"stale wal").expect("stale wal");

        recover_database_from_path_at(&backup, &data).expect("recovery succeeds");

        assert!(
            !data.join("medical.db.recovery-tmp").exists(),
            "tmp copy renamed into place"
        );
        assert!(
            !data.join("medical.db-wal").exists(),
            "stale sidecar removed after the swap"
        );
        assert!(
            !medical_db::encryption::is_plaintext_db(&data.join("medical.db"))
                .expect("inspect result"),
            "the swapped-in DB must be migrated to encrypted"
        );
    }

    /// The non-plaintext guard still rejects encrypted/garbage backups
    /// before anything is touched.
    #[test]
    fn non_plaintext_backup_is_rejected_up_front() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let backup = tmp.path().join("ciphertext.db");
        std::fs::write(&backup, b"ciphertext bytes without the sqlite magic").expect("write");
        let data = tmp.path().join("data");

        let result = recover_database_from_path_at(&backup, &data);
        assert!(result.is_err());
        assert!(
            !data.join("medical.db").exists(),
            "nothing is installed for a rejected backup"
        );
    }
}
