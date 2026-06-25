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
#[tauri::command]
pub async fn recover_database_from_path(backup_path: String) -> AppResult<()> {
    let backup = PathBuf::from(&backup_path);
    if !backup.exists() {
        return Err(AppError::Other(format!(
            "backup file not found: {backup_path}"
        )));
    }
    if !medical_db::encryption::is_plaintext_db(&backup)
        .map_err(|e| AppError::Other(format!("inspect backup: {e}")))?
    {
        return Err(AppError::Other(
            "Selected file is not a plaintext SQLite database. Encrypted backups \
             cannot be restored without the original keychain entry."
                .into(),
        ));
    }

    let data_dir = data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| AppError::Other(format!("create data dir: {e}")))?;
    let db_path = data_dir.join("medical.db");

    // Wipe stale state.
    medical_security::keychain::wipe_db_key()
        .map_err(|e| AppError::Other(format!("clear keychain: {e}")))?;
    if db_path.exists() {
        let _ = std::fs::remove_file(&db_path);
    }
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));

    // Copy the picked backup into place.
    std::fs::copy(&backup, &db_path).map_err(|e| AppError::Other(format!("copy backup: {e}")))?;

    // Generate a fresh key and migrate.
    let key = medical_security::keychain::get_or_create_db_key()
        .map_err(|e| AppError::Other(format!("create key: {e}")))?;
    medical_db::encryption::migrate_plaintext_to_encrypted(&db_path, &key)
        .map_err(|e| AppError::Other(format!("migration: {e}")))?;

    Ok(())
}

/// Wipe the encrypted DB and the keychain entry. Frontend should reload.
#[tauri::command]
pub async fn recover_database_wipe() -> AppResult<()> {
    let data_dir = data_dir()?;
    let db_path = data_dir.join("medical.db");

    medical_security::keychain::wipe_db_key()
        .map_err(|e| AppError::Other(format!("wipe keychain: {e}")))?;
    if db_path.exists() {
        std::fs::remove_file(&db_path).map_err(|e| AppError::Other(format!("remove db: {e}")))?;
    }
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));

    Ok(())
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

    let plaintext = medical_db::encryption::is_plaintext_db(&db_path)
        .map_err(|e| AppError::Other(format!("inspect: {e}")))?;
    let key_present = medical_security::keychain::get_db_key()
        .map(|opt| opt.is_some())
        .unwrap_or(false);

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
