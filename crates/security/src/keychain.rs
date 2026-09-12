//! Cross-platform OS-keychain wrapper for the database encryption key.
//!
//! Uses the `keyring` crate v3 with platform-native backends:
//! - macOS: Security framework (Keychain Services) via `apple-native`
//! - Windows: Credential Manager via `windows-native`
//! - Linux: libsecret / Secret Service via `sync-secret-service`
//!
//! This module manages 32-byte secrets under service `rustMedicalAssistant`:
//! the SQLCipher database encryption key (account `db-key`) and the backup
//! wrapping key (account `backup-wrapping-key`, added by the off-machine
//! backup tool — kept separate so wiping the DB key never destroys the
//! ability to restore old snapshots). Other secrets (API keys, sharing
//! tokens) live in [`crate::key_storage::KeyStorage`].
//!
//! The sharing crate reuses this same keychain entry as the sharing-store
//! encryption key so there is only one OS-level secret to manage per
//! install. See the README "Cross-Crate Contracts" section.
//!
//! # Testing
//!
//! Tests install a **global** mock provider that covers all threads,
//! spawned tasks, and cross-crate dependencies. Call
//! `set_test_provider(provider)` before the code under test; call
//! `clear_test_provider()` (or use the RAII `with_test_provider()`) to
//! restore the real OS keychain.
//!
//! When a test provider is installed, **no OS keychain call is made** —
//! the `OsKeychain` backend is unreachable. If a test binary reaches
//! `OsKeychain` without a provider, it panics immediately instead of
//! blocking on a security prompt.
//!
//! Example:
//! ```rust
//! #[test]
//! fn my_test() {
//!     keychain::set_test_provider(keychain::TestProvider::fixed([42u8; 32]));
//!     let key = keychain::get_db_key().unwrap();
//!     assert_eq!(key, Some([42u8; 32]));
//!     keychain::clear_test_provider();
//! }
//! ```

use keyring::Entry;
use rand::RngCore;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Trait for secret storage backends. Production uses the OS keychain;
/// tests inject a mock via `set_test_provider()`.
pub trait SecretProvider: Send + Sync {
    /// Retrieve a 32-byte secret by account name. Returns `Ok(None)` if
    /// no entry exists, `Ok(Some(key))` if found, or an error on access failure.
    fn get_secret(&self, account: &str) -> KeychainResult<Option<[u8; 32]>>;

    /// Store a 32-byte secret under the given account name.
    fn set_secret(&self, account: &str, key: [u8; 32]) -> KeychainResult<()>;

    /// Delete the secret for the given account. Idempotent: returns `Ok(())`
    /// if no entry exists.
    fn delete_secret(&self, account: &str) -> KeychainResult<()>;
}

// ── Global provider state ──────────────────────────────────────────────
// RwLock<Option<Arc<dyn SecretProvider>>> — read-hot (every keychain call
// reads this), write-cold (only tests swap it). All threads and spawned
// tasks see the same provider; no thread-local leakage.

lazy_static::lazy_static! {
    static ref GLOBAL_PROVIDER: RwLock<Option<Arc<dyn SecretProvider>>> =
        RwLock::new(None);
}

/// Install a test provider **globally**. Every thread, spawned task, and
/// cross-crate dependency sees this provider until `clear_test_provider()`
/// is called. This is the correct isolation for `cargo test` where
/// `medical-security` is compiled as a dependency of another crate's
/// tests — the `#[cfg(test)]` guard on `OsKeychain` does not fire in
/// that case, but the global provider still routes correctly.
pub fn set_test_provider(provider: impl SecretProvider + 'static) {
    let mut guard = GLOBAL_PROVIDER.write().unwrap();
    *guard = Some(Arc::new(provider));
}

/// Remove the test provider, reverting to the real OS keychain.
pub fn clear_test_provider() {
    let mut guard = GLOBAL_PROVIDER.write().unwrap();
    *guard = None;
}

/// Run a closure with the test provider active, then automatically clear
/// it (RAII). Panics in the closure still clear the provider.
pub fn with_test_provider<F, R>(provider: impl SecretProvider + 'static, f: F) -> R
where
    F: FnOnce() -> R,
{
    set_test_provider(provider);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    clear_test_provider();
    match result {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// Returns `true` if a test provider is currently installed globally.
/// Useful for test-only assertions that verify isolation is active.
pub fn is_test_provider_active() -> bool {
    GLOBAL_PROVIDER.read().unwrap().is_some()
}

/// In-memory mock provider for tests. Stores secrets in a HashMap.
pub struct TestProvider {
    secrets: Mutex<HashMap<String, [u8; 32]>>,
}

impl TestProvider {
    /// Create a provider that returns `None` for all lookups (empty keychain).
    pub fn empty() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
        }
    }

    /// Create a provider that returns `fixed_key` for the DB key account
    /// and `None` for all others.
    pub fn fixed_db_key(fixed_key: [u8; 32]) -> Self {
        let mut secrets = HashMap::new();
        secrets.insert(KEYCHAIN_DB_KEY_ACCOUNT.to_string(), fixed_key);
        Self {
            secrets: Mutex::new(secrets),
        }
    }

    /// Create a provider pre-loaded with the given secrets.
    pub fn with_secrets(secrets: HashMap<String, [u8; 32]>) -> Self {
        Self {
            secrets: Mutex::new(secrets),
        }
    }
}

impl SecretProvider for TestProvider {
    fn get_secret(&self, account: &str) -> KeychainResult<Option<[u8; 32]>> {
        let guard = self.secrets.lock().unwrap();
        Ok(guard.get(account).copied())
    }

    fn set_secret(&self, account: &str, key: [u8; 32]) -> KeychainResult<()> {
        let mut guard = self.secrets.lock().unwrap();
        guard.insert(account.to_string(), key);
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> KeychainResult<()> {
        let mut guard = self.secrets.lock().unwrap();
        guard.remove(account);
        Ok(())
    }
}

/// Internal helper: route to the global test provider if installed,
/// otherwise use `OsKeychain`.
fn with_provider<F, R>(f: F) -> R
where
    F: FnOnce(&dyn SecretProvider) -> R,
{
    let guard = GLOBAL_PROVIDER.read().unwrap();
    if let Some(provider) = guard.as_ref() {
        f(provider.as_ref())
    } else {
        // Drop the read-lock before calling into OsKeychain — it may
        // block on a prompt and we must not hold the lock across that.
        drop(guard);
        f(&OsKeychain)
    }
}

/// Production backend: the real OS keychain via the `keyring` crate.
struct OsKeychain;

impl SecretProvider for OsKeychain {
    fn get_secret(&self, account: &str) -> KeychainResult<Option<[u8; 32]>> {
        // Fail-fast guard: if we're running in a test binary (detected by
        // the presence of `cargo test` or `cargo nextest` in the process
        // args) and no test provider is installed, panic instead of
        // blocking on a keychain prompt. This catches cross-crate
        // dependencies that compile `medical-security` without `#[cfg(test)]`.
        if is_running_in_test_context() {
            panic!(
                "OsKeychain::get_secret called in test context without a test provider installed. \
                 Call keychain::set_test_provider() before running code that touches the keychain."
            );
        }

        let entry = Entry::new(KEYCHAIN_SERVICE, account)
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

    fn set_secret(&self, account: &str, key: [u8; 32]) -> KeychainResult<()> {
        if is_running_in_test_context() {
            panic!(
                "OsKeychain::set_secret called in test context without a test provider installed."
            );
        }

        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        entry
            .set_secret(&key)
            .map_err(|e| KeychainError::Access(e.to_string()))
    }

    fn delete_secret(&self, account: &str) -> KeychainResult<()> {
        if is_running_in_test_context() {
            panic!(
                "OsKeychain::delete_secret called in test context without a test provider installed."
            );
        }

        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::Access(e.to_string())),
        }
    }
}

/// Detect whether the current process is a test binary by inspecting
/// environment variables set by `cargo test` / `cargo nextest` / `pytest`
/// runners. This is not `#[cfg(test)]` — that only fires for the crate
/// being tested, not its dependencies.
fn is_running_in_test_context() -> bool {
    // cargo test sets RUST_TEST_THREADS; cargo nextest sets NEXTEST
    std::env::var("RUST_TEST_THREADS").is_ok()
        || std::env::var("NEXTEST").is_ok()
        // Integration test binaries often have `test` in argv[0]
        || std::env::args().next().map_or(false, |arg0| arg0.contains("test"))
}

/// Service name used in the OS keychain.
///
/// Exposed for tests and manual inspection (e.g.
/// `security find-generic-password -s rustMedicalAssistant -a db-key`
/// on macOS).
pub const KEYCHAIN_SERVICE: &str = "rustMedicalAssistant";
/// Account name used to identify the database encryption key within the
/// [`KEYCHAIN_SERVICE`] service.
pub const KEYCHAIN_DB_KEY_ACCOUNT: &str = "db-key";
/// Account name for the backup wrapping key — the 32-byte root key whose
/// only copies off-machine are the escrow artifacts (recovery sheet +
/// offline USB) produced by the `medical-backup` tool. Distinct from
/// `db-key` so a "wipe and start fresh" of the DB key never destroys the
/// ability to restore old snapshots.
pub const KEYCHAIN_BACKUP_KEY_ACCOUNT: &str = "backup-wrapping-key";

/// Errors returned by the keychain wrapper.
#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("keychain access denied or unavailable: {0}")]
    Access(String),
    #[error("keychain entry malformed: {0}")]
    Malformed(String),
    // Entropy is no longer constructible after the rand 0.9 migration
    // (fill_bytes is infallible on ThreadRng). Retained for API/doc stability.
    #[allow(dead_code)]
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
    get_secret(KEYCHAIN_DB_KEY_ACCOUNT)
}

/// Read a 32-byte secret from the OS keychain under `account`.
///
/// Returns `Ok(None)` if no entry exists. Shared implementation behind
/// [`get_db_key`] and the backup wrapping-key lookup in `medical-backup`.
pub fn get_secret(account: &str) -> KeychainResult<Option<[u8; 32]>> {
    with_provider(|p| p.get_secret(account))
}

/// Store a 32-byte secret under `account`, creating or replacing the entry.
pub fn set_secret(account: &str, key: [u8; 32]) -> KeychainResult<()> {
    with_provider(|p| p.set_secret(account, key))
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
    get_or_create_secret(KEYCHAIN_DB_KEY_ACCOUNT)
}

/// Get the existing 32-byte secret under `account`, or generate and store
/// a new random key if none exists yet. Shared implementation behind
/// [`get_or_create_db_key`] and the backup wrapping-key bootstrap.
pub fn get_or_create_secret(account: &str) -> KeychainResult<[u8; 32]> {
    if let Some(key) = get_secret(account)? {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);
    set_secret(account, key)?;
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
    with_provider(|p| p.delete_secret(KEYCHAIN_DB_KEY_ACCOUNT))
}

/// Encode a 32-byte key as a 64-character lowercase hex string suitable
/// for SQLCipher's `PRAGMA key="x'<hex>'"` syntax.
pub fn key_to_hex(key: &[u8; 32]) -> String {
    hex::encode(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    // Serialize tests that mutate the global provider — cargo test runs
    // lib tests in parallel by default, which would cause races.
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn get_db_key_returns_none_when_absent() {
        let _guard = serial_lock();
        set_test_provider(TestProvider::empty());
        let result = get_db_key().expect("read");
        assert!(result.is_none(), "expected None on empty keychain, got Some");
        clear_test_provider();
    }

    #[test]
    fn get_db_key_returns_fixed_key_when_set() {
        let _guard = serial_lock();
        let fixed = [42u8; 32];
        set_test_provider(TestProvider::fixed_db_key(fixed));
        let result = get_db_key().expect("read");
        assert_eq!(result, Some(fixed));
        clear_test_provider();
    }

    #[test]
    fn get_or_create_persists_across_calls() {
        let _guard = serial_lock();
        set_test_provider(TestProvider::empty());
        let first = get_or_create_db_key().expect("first call");
        let second = get_or_create_db_key().expect("second call");
        assert_eq!(first, second, "should return the same key on subsequent calls");
        clear_test_provider();
    }

    #[test]
    fn set_and_get_roundtrips() {
        let _guard = serial_lock();
        set_test_provider(TestProvider::empty());
        let key = [99u8; 32];
        set_secret("test-account", key).expect("set");
        let retrieved = get_secret("test-account").expect("get");
        assert_eq!(retrieved, Some(key));
        clear_test_provider();
    }

    #[test]
    fn wipe_removes_secret() {
        let _guard = serial_lock();
        set_test_provider(TestProvider::empty());
        let key = [77u8; 32];
        set_secret(KEYCHAIN_DB_KEY_ACCOUNT, key).expect("set");
        assert!(get_db_key().expect("get").is_some());

        wipe_db_key().expect("wipe");
        assert!(get_db_key().expect("get after wipe").is_none());
        clear_test_provider();
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

    #[test]
    fn with_test_provider_scopes_correctly() {
        let _guard = serial_lock();
        let key = [55u8; 32];
        let result = with_test_provider(TestProvider::fixed_db_key(key), || {
            get_db_key().expect("inside scope")
        });
        assert_eq!(result, Some(key));
    }

    #[test]
    fn is_test_provider_active_reflects_state() {
        let _guard = serial_lock();
        assert!(!is_test_provider_active());
        set_test_provider(TestProvider::empty());
        assert!(is_test_provider_active());
        clear_test_provider();
        assert!(!is_test_provider_active());
    }
}
