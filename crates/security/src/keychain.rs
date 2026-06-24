//! Cross-platform OS-keychain wrapper for the database encryption key.
//!
//! Uses the `keyring` crate v3 with platform-native backends:
//! - macOS: Security framework (Keychain Services) via `apple-native`
//! - Windows: Credential Manager via `windows-native`
//! - Linux: libsecret / Secret Service via `sync-secret-service`
//!
//! This module manages a **single** 32-byte secret — the SQLCipher database
//! encryption key — stored under service `rustMedicalAssistant` / account
//! `db-key`. Other secrets (API keys, sharing tokens) live in
//! [`crate::key_storage::KeyStorage`]; do not add new keychain entries
//! here without coordinating with the recovery path in
//! `src-tauri/src/commands/recovery.rs`.
//!
//! The sharing crate reuses this same keychain entry as the sharing-store
//! encryption key so there is only one OS-level secret to manage per
//! install. See the README "Cross-Crate Contracts" section.
//!
//! # Testing
//!
//! For tests, call `keyring::set_default_credential_builder(...)` with the
//! mock builder before invoking these functions to isolate test runs. Note
//! the mock backend is `EntryOnly`: every `Entry::new()` returns a fresh
//! empty credential, so cross-call persistence cannot be unit-tested —
//! that is covered by integration tests and manual smoke testing.

use keyring::Entry;
use rand::RngCore;

/// Service name used in the OS keychain.
///
/// Exposed for tests and manual inspection (e.g.
/// `security find-generic-password -s rustMedicalAssistant -a db-key`
/// on macOS).
pub const KEYCHAIN_SERVICE: &str = "rustMedicalAssistant";
/// Account name used to identify the database encryption key within the
/// [`KEYCHAIN_SERVICE`] service.
pub const KEYCHAIN_DB_KEY_ACCOUNT: &str = "db-key";

/// Errors returned by the keychain wrapper.
#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("keychain access denied or unavailable: {0}")]
    Access(String),
    #[error("keychain entry malformed: {0}")]
    Malformed(String),
    #[error("entropy source failed: {0}")]
    Entropy(String),
}

pub type KeychainResult<T> = Result<T, KeychainError>;

/// Read the database key from the OS keychain.
///
/// Returns `Ok(None)` if no entry exists yet — callers that need a key
/// should typically use [`get_or_create_db_key`] instead, which handles
/// the first-run case automatically.
///
/// # Errors
///
/// - [`KeychainError::Access`] if the OS keychain is locked, the user
///   denies the access prompt, or the keyring library otherwise fails.
/// - [`KeychainError::Malformed`] if the stored entry is not exactly 32
///   bytes (indicates corruption or a different writer).
pub fn get_db_key() -> KeychainResult<Option<[u8; 32]>> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_DB_KEY_ACCOUNT)
        .map_err(|e| KeychainError::Access(e.to_string()))?;
    match entry.get_secret() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(KeychainError::Malformed(format!(
                    "expected 32 bytes, got {}",
                    bytes.len()
                )));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(Some(key))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeychainError::Access(e.to_string())),
    }
}

/// Get the existing database key from the keychain, or generate and store
/// a new random 32-byte key if none exists yet.
///
/// This is the recommended entry point for app startup — `AppState::initialize`
/// calls it to obtain the SQLCipher key, and the sharing crate reuses the
/// same value as the sharing-store encryption key.
///
/// # Errors
///
/// - [`KeychainError::Access`] on any OS keychain failure.
/// - [`KeychainError::Entropy`] if the platform RNG cannot produce 32
///   random bytes (effectively impossible on a functioning OS).
pub fn get_or_create_db_key() -> KeychainResult<[u8; 32]> {
    if let Some(key) = get_db_key()? {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng()
        .try_fill_bytes(&mut key)
        .map_err(|e| KeychainError::Entropy(e.to_string()))?;
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_DB_KEY_ACCOUNT)
        .map_err(|e| KeychainError::Access(e.to_string()))?;
    entry
        .set_secret(&key)
        .map_err(|e| KeychainError::Access(e.to_string()))?;
    Ok(key)
}

/// Remove the database key from the keychain.
///
/// Used by the "Wipe and start fresh" recovery path in
/// `src-tauri/src/commands/recovery.rs`. After calling this, the
/// encrypted database is unrecoverable — the next startup will generate
/// a fresh key and an empty database.
///
/// Idempotent: returns `Ok(())` if no entry exists.
///
/// # Errors
///
/// - [`KeychainError::Access`] on OS keychain failure (other than
///   "entry not found", which is treated as success).
pub fn wipe_db_key() -> KeychainResult<()> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_DB_KEY_ACCOUNT)
        .map_err(|e| KeychainError::Access(e.to_string()))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(KeychainError::Access(e.to_string())),
    }
}

/// Encode a 32-byte key as a 64-character lowercase hex string suitable
/// for SQLCipher's `PRAGMA key="x'<hex>'"` syntax.
pub fn key_to_hex(key: &[u8; 32]) -> String {
    hex::encode(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configure keyring to use the in-process mock backend so tests don't
    /// touch the real OS keychain. Note that the mock is `EntryOnly`
    /// persistence — every call to `Entry::new(SERVICE, ACCOUNT)` returns a
    /// fresh empty credential rather than sharing state. That makes
    /// cross-call persistence tests impossible to write at the unit level;
    /// real persistence is verified by the integration tests in Task 4 and
    /// by manual smoke testing on each platform.
    fn use_mock_backend() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }

    #[test]
    fn get_db_key_returns_none_when_absent() {
        use_mock_backend();
        // Each Entry::new() in the mock backend yields a fresh empty
        // credential, so any first read sees NoEntry → our wrapper maps
        // that to Ok(None).
        let result = get_db_key().expect("read");
        assert!(
            result.is_none(),
            "expected None on empty keychain, got Some"
        );
    }

    #[test]
    fn key_to_hex_produces_64_chars() {
        let key = [0xABu8; 32];
        let hex_str = key_to_hex(&key);
        assert_eq!(hex_str.len(), 64);
        assert_eq!(
            hex_str,
            "abababababababababababababababababababababababababababababababab"
        );
    }
}
