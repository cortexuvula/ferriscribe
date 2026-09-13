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
//! fn scoped_mock() {
//!     medical_security::keychain::set_test_provider(
//!         medical_security::keychain::TestProvider::fixed_db_key([42u8; 32]),
//!     );
//!     let key = medical_security::keychain::get_db_key().unwrap();
//!     assert_eq!(key, Some([42u8; 32]));
//!     medical_security::keychain::clear_test_provider();
//! }
//! scoped_mock();
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
    // Poison-tolerant: if a panic ever strikes while this RwLock is held
    // (a sentinel firing mid-swap, a test aborting between install and
    // clear), a poisoned lock must not convert every LATER set/clear into
    // a panic cascade while other test locks are held. Recover the guard;
    // the provider slot itself is still consistent (full-slot writes only).
    let mut guard = GLOBAL_PROVIDER.write().unwrap_or_else(|e| e.into_inner());
    *guard = Some(Arc::new(provider));
}

/// Remove the test provider, reverting to the real OS keychain.
pub fn clear_test_provider() {
    // Poison-tolerant for the same reason as `set_test_provider`.
    let mut guard = GLOBAL_PROVIDER.write().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// Run a closure with the test provider active, then automatically clear
/// it (RAII). Panics in the closure still clear the provider.
///
/// Callers that touch a **file-backed** database must pair this with a
/// temporary DB path (see the module docs on key/DB pairing): the mock's
/// synthetic key only decrypts a DB that was created under it, so an
/// accidental open of the real `medical.db` fails on open instead of
/// silently succeeding.
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
    GLOBAL_PROVIDER
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
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
/// otherwise use `OsKeychain` (production) or `TestSentinel` (test builds).
///
/// The test-detection is **belt and braces**: both a `#[cfg(test)]` compile
/// gate and a runtime harness check. The compile gate covers unit tests in
/// this crate; the runtime check covers the case where this crate is
/// compiled as a *dependency* of another crate's test binary — there
/// `cfg(test)` is false for this crate, `OsKeychain` is compiled in, and
/// without the runtime check a missing provider would silently reach the
/// real OS keychain (and on macOS block forever on a securityd prompt).
fn with_provider<F, R>(f: F) -> R
where
    F: FnOnce(&dyn SecretProvider) -> R,
{
    // Snapshot the Arc under the read-lock, then drop the lock before
    // calling the closure. This prevents deadlocks if the closure
    // spawns threads that call clear_test_provider() (which needs a
    // write-lock).
    let provider_arc: Option<Arc<dyn SecretProvider>> = {
        let guard = GLOBAL_PROVIDER.read().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };

    if let Some(provider) = provider_arc {
        f(provider.as_ref())
    } else if cfg!(test) || running_under_test_harness() {
        f(&TestSentinel)
    } else {
        f(&OsKeychain)
    }
}

/// True when the current process looks like a Rust test harness.
///
/// Detection signals (any one suffices):
/// - `NEXTEST=1` — cargo-nextest sets this in every test process.
/// - `RUST_TEST_THREADS` is set — `cargo test` and most harness wrappers
///   set it; a production app has no reason to.
/// - `argv[0]`'s file name matches cargo's test-binary layout
///   (`deps/<name>-<16 hex chars>`) or ends with `.d`/`-<hash>` smoke-test
///   names libtest uses when invoking a binary's `--list` self-check.
///
/// The result is cached in a `OnceLock`: argv and the environment are
/// stable for the process lifetime, and this is on the hot path of every
/// keychain call that has no provider installed (production startup).
///
/// False positives (a production process that happens to define
/// `RUST_TEST_THREADS`) trade a panic for what would otherwise be a real
/// keychain access — fail-closed is the intended direction. False
/// negatives fall back to `OsKeychain`, exactly like production.
fn running_under_test_harness() -> bool {
    static DETECTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DETECTED.get_or_init(|| {
        let argv0 = std::env::args()
            .next()
            .as_deref()
            .and_then(|a| std::path::Path::new(a).file_name())
            .and_then(|n| n.to_str())
            .map(str::to_owned);
        detect_harness_signals(
            std::env::var_os("NEXTEST").as_deref(),
            std::env::var_os("RUST_TEST_THREADS").as_deref(),
            argv0.as_deref(),
        )
    })
}

/// Pure decision core of [`running_under_test_harness`] — separated so the
/// detection logic (including the must-NOT-fire cases) is unit-testable
/// without manipulating the real process environment.
fn detect_harness_signals(
    nextest: Option<&std::ffi::OsStr>,
    rust_test_threads: Option<&std::ffi::OsStr>,
    argv0: Option<&str>,
) -> bool {
    if nextest.is_some_and(|v| v == "1") {
        return true;
    }
    if rust_test_threads.is_some() {
        return true;
    }
    argv0.is_some_and(harness_binary_name)
}

/// Classify an `argv[0]` file name as a cargo-built test binary.
fn harness_binary_name(name: &str) -> bool {
    // Cargo's test artifacts are `deps/<crate-or-bin>-<16 hex chars>`
    // (no extension on macOS/Linux, `.exe` on Windows). Match the suffix
    // shape after stripping a platform extension.
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    let Some((prefix, suffix)) = stem.rsplit_once('-') else {
        return false;
    };
    !prefix.is_empty() && suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Fail-fast backend used when a test-context process reaches a keychain
/// operation with no provider installed. Panics instead of touching the
/// real OS keychain: in a test there is no legitimate reason to read the
/// operator's keychain, and on macOS an access prompt would hang the
/// whole harness (the original bug this module's isolation exists to
/// prevent — `SecKeychainFindGenericPassword` blocking indefinitely).
///
/// Compiled unconditionally (not `#[cfg(test)]`): when this crate is a
/// dependency of another crate's test binary, `cfg(test)` is false here
/// and only the runtime harness check in [`with_provider`] routes here.
struct TestSentinel;

impl SecretProvider for TestSentinel {
    fn get_secret(&self, account: &str) -> KeychainResult<Option<[u8; 32]>> {
        panic!(
            "OsKeychain::get_secret({account:?}) reached in a test harness without a test \
             provider. Call set_test_provider() before accessing the keychain in tests."
        );
    }
    fn set_secret(&self, account: &str, _key: [u8; 32]) -> KeychainResult<()> {
        panic!(
            "OsKeychain::set_secret({account:?}) reached in a test harness without a test \
             provider. Call set_test_provider() before accessing the keychain in tests."
        );
    }
    fn delete_secret(&self, account: &str) -> KeychainResult<()> {
        panic!(
            "OsKeychain::delete_secret({account:?}) reached in a test harness without a test \
             provider. Call set_test_provider() before accessing the keychain in tests."
        );
    }
}

#[cfg(test)]
mod harness_detection_tests {
    use super::*;

    #[test]
    fn harness_binary_name_matches_cargo_test_layout() {
        // Exact cargo layout: deps/<name>-<16 hex>
        assert!(harness_binary_name(
            "rust_medical_assistant_lib-9f1c2b3d4e5f6071"
        ));
        assert!(harness_binary_name("medical_security-abcdef0123456789"));
        assert!(harness_binary_name("medical_security-ABCDEF0123456789.exe"));
        // NOT harness binaries:
        assert!(!harness_binary_name(
            "ferriscribe-backup-aarch64-apple-darwin"
        ));
        assert!(!harness_binary_name("rust-medical-assistant")); // no hash suffix
        assert!(!harness_binary_name("app-9f1c2b3")); // hash too short
        assert!(!harness_binary_name("app-9f1c2b3d4e5f60718")); // 17 chars
        assert!(!harness_binary_name(
            "app-zzzzzzzzzzzzzzzz".replace('z', "g").as_str()
        )); // non-hex
        assert!(!harness_binary_name("-0123456789abcdef")); // empty prefix
    }

    #[test]
    fn running_under_test_harness_is_true_in_this_binary() {
        // This test IS running under a harness (cargo test sets
        // RUST_TEST_THREADS, and argv[0] is a deps/<name>-<hash> binary),
        // so the detection this test exists to verify must fire here.
        assert!(
            running_under_test_harness(),
            "detection must recognise the current cargo test process"
        );
    }

    #[test]
    fn detection_fires_for_each_harness_signal() {
        use std::ffi::OsStr;
        let prod_name = "/Applications/FerriScribe.app/Contents/MacOS/rust-medical-assistant";
        // NEXTEST=1 alone
        assert!(detect_harness_signals(
            Some(OsStr::new("1")),
            None,
            Some(prod_name)
        ));
        // RUST_TEST_THREADS set (any value) alone
        assert!(detect_harness_signals(
            None,
            Some(OsStr::new("4")),
            Some(prod_name)
        ));
        // cargo test-binary argv[0] alone
        assert!(detect_harness_signals(
            None,
            None,
            Some("/target/debug/deps/rust_medical_assistant_lib-9f1c2b3d4e5f6071")
        ));
        assert!(detect_harness_signals(
            None,
            None,
            Some("C:\\ci\\deps\\app_lib-0123456789abcdef.exe")
        ));
    }

    #[test]
    fn detection_does_not_fire_for_production_processes() {
        use std::ffi::OsStr;
        // The MUST-NOT-FIRE cases: a production app launched normally, a
        // bundled helper binary, a CLI run by hand — none of these may be
        // misclassified as a test harness (they'd panic on keychain use).
        let none: Option<&OsStr> = None;
        for prod in [
            "/Applications/FerriScribe.app/Contents/MacOS/rust-medical-assistant",
            "/usr/local/bin/ferriscribe-backup-aarch64-apple-darwin",
            "/home/andre/.cargo/bin/some-tool",
            "rust-medical-assistant",
            "./target/release/rust-medical-assistant",
        ] {
            assert!(
                !detect_harness_signals(none, none, Some(prod)),
                "production argv[0] must NOT be classified as a test harness: {prod}"
            );
        }
        // NEXTEST set but not "1" (e.g. NEXTEST_PROFILE leaks into a
        // spawned prod process env by prefix collision — value must be
        // exactly "1")
        assert!(!detect_harness_signals(
            Some(OsStr::new("0")),
            none,
            Some("/Applications/App.app/Contents/MacOS/app")
        ));
        // No argv[0] at all (paranoia; args() is never empty in practice)
        assert!(!detect_harness_signals(none, none, None));
    }

    #[test]
    fn sentinel_panics_for_every_operation() {
        // Positive path: with no provider installed, each operation must
        // fail fast rather than reach the OS keychain.
        assert!(std::panic::catch_unwind(|| TestSentinel.get_secret("db-key")).is_err());
        assert!(std::panic::catch_unwind(|| TestSentinel.set_secret("db-key", [0u8; 32])).is_err());
        assert!(std::panic::catch_unwind(|| TestSentinel.delete_secret("db-key")).is_err());
    }

    #[test]
    fn string_secrets_round_trip_through_the_mock() {
        let _guard = serial_lock();
        set_test_string_provider(TestStringProvider::empty());
        // Synthetic fixture token only — never real key material.
        set_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT, "fixture-token-7f3a")
            .expect("set string");
        assert_eq!(
            get_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT).expect("get string"),
            Some("fixture-token-7f3a".to_string())
        );
        delete_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT).expect("delete string");
        assert_eq!(
            get_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT).expect("get after delete"),
            None,
            "delete must remove the string secret"
        );
        clear_test_string_provider();
    }

    #[test]
    fn string_secret_delete_is_idempotent() {
        let _guard = serial_lock();
        set_test_string_provider(TestStringProvider::empty());
        // Deleting a never-set account must succeed (unpair is idempotent).
        delete_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT).expect("idempotent delete");
        clear_test_string_provider();
    }

    #[test]
    fn string_set_and_delete_without_provider_fail_fast() {
        let _guard = serial_lock();
        assert!(
            !is_test_string_provider_active(),
            "precondition: no provider"
        );
        // MUTATING operations must panic via the sentinel, never reach the
        // OS keychain.
        assert!(
            std::panic::catch_unwind(|| {
                set_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT, "fixture")
            })
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| delete_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT))
                .is_err()
        );
        // The READ is deliberately lenient (Ok(None) = "not paired") — see
        // get_string_secret's doc for why incidental consumers require it.
        assert_eq!(
            get_string_secret(KEYCHAIN_SHARING_BEARER_ACCOUNT).expect("lenient read"),
            None
        );
    }
}

/// Production backend: the real OS keychain via the `keyring` crate.
///
/// Compiled unconditionally so `with_provider` can name it in both build
/// modes; test contexts never reach it (provider installed, `cfg!(test)`,
/// or the runtime harness check route elsewhere first).
struct OsKeychain;

impl SecretProvider for OsKeychain {
    fn get_secret(&self, account: &str) -> KeychainResult<Option<[u8; 32]>> {
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
        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        entry
            .set_secret(&key)
            .map_err(|e| KeychainError::Access(e.to_string()))
    }

    fn delete_secret(&self, account: &str) -> KeychainResult<()> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::Access(e.to_string())),
        }
    }
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
// ── String secrets (sharing-bearer) ─────────────────────────────────────
//
// The 32-byte SecretProvider trait covers the DB/backup keys. The sharing
// bearer is an opaque UTF-8 token (generated by the office server), so it
// cannot ride the byte-trait without a lossy re-encoding contract. It gets
// the same provider routing via a parallel trait object: test doubles and
// the runtime harness detection cover it identically, and the sentinel
// fires for it too.

/// Trait for string-secret storage backends. Production uses the OS
/// keychain (`OsStringSecretKeychain`); tests inject a mock via
/// [`set_test_string_provider`].
pub trait StringSecretProvider: Send + Sync {
    /// Retrieve a UTF-8 secret by account name. `Ok(None)` = no entry.
    fn get_string(&self, account: &str) -> KeychainResult<Option<String>>;

    /// Store a UTF-8 secret under `account`, creating or replacing it.
    fn set_string(&self, account: &str, secret: &str) -> KeychainResult<()>;

    /// Delete the secret for `account`. Idempotent: `Ok(())` if absent.
    fn delete_string(&self, account: &str) -> KeychainResult<()>;
}

lazy_static::lazy_static! {
    static ref GLOBAL_STRING_PROVIDER: RwLock<Option<Arc<dyn StringSecretProvider>>> =
        RwLock::new(None);
}

/// Install a global test provider for string secrets (see
/// [`set_test_provider`] for the byte-secret equivalent).
pub fn set_test_string_provider(provider: impl StringSecretProvider + 'static) {
    // Poison-tolerant for the same reason as the byte-secret variant.
    let mut guard = GLOBAL_STRING_PROVIDER
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *guard = Some(Arc::new(provider));
}

/// Remove the string-secret test provider.
pub fn clear_test_string_provider() {
    let mut guard = GLOBAL_STRING_PROVIDER
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// True while a string-secret test provider is installed.
pub fn is_test_string_provider_active() -> bool {
    GLOBAL_STRING_PROVIDER
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

/// In-memory mock for string secrets (HashMap-backed, same shape as
/// [`TestProvider`]).
pub struct TestStringProvider {
    secrets: Mutex<HashMap<String, String>>,
}

impl TestStringProvider {
    /// Empty string keychain: every lookup returns `None`.
    pub fn empty() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
        }
    }

    /// Provider pre-loaded with `account -> secret` mappings.
    pub fn with_secrets(secrets: HashMap<String, String>) -> Self {
        Self {
            secrets: Mutex::new(secrets),
        }
    }
}

impl StringSecretProvider for TestStringProvider {
    fn get_string(&self, account: &str) -> KeychainResult<Option<String>> {
        let guard = self.secrets.lock().unwrap();
        Ok(guard.get(account).cloned())
    }

    fn set_string(&self, account: &str, secret: &str) -> KeychainResult<()> {
        let mut guard = self.secrets.lock().unwrap();
        guard.insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete_string(&self, account: &str) -> KeychainResult<()> {
        let mut guard = self.secrets.lock().unwrap();
        guard.remove(account);
        Ok(())
    }
}

/// Fail-fast sentinel for string secrets — same contract as [`TestSentinel`].
struct TestStringSentinel;

impl StringSecretProvider for TestStringSentinel {
    fn get_string(&self, account: &str) -> KeychainResult<Option<String>> {
        panic!(
            "OsStringSecretKeychain::get_string({account:?}) reached in a test harness without \
             a test provider. Call set_test_string_provider() before accessing the keychain \
             in tests."
        );
    }
    fn set_string(&self, account: &str, _secret: &str) -> KeychainResult<()> {
        panic!(
            "OsStringSecretKeychain::set_string({account:?}) reached in a test harness without \
             a test provider. Call set_test_string_provider() before accessing the keychain \
             in tests."
        );
    }
    fn delete_string(&self, account: &str) -> KeychainResult<()> {
        panic!(
            "OsStringSecretKeychain::delete_string({account:?}) reached in a test harness without \
             a test provider. Call set_test_string_provider() before accessing the keychain \
             in tests."
        );
    }
}

/// Production string-secret backend: the real OS keychain. Same routing
/// rules as `OsKeychain` — compiled unconditionally, unreachable in test
/// contexts.
struct OsStringSecretKeychain;

impl StringSecretProvider for OsStringSecretKeychain {
    fn get_string(&self, account: &str) -> KeychainResult<Option<String>> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeychainError::Access(e.to_string())),
        }
    }

    fn set_string(&self, account: &str, secret: &str) -> KeychainResult<()> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|e| KeychainError::Access(e.to_string()))
    }

    fn delete_string(&self, account: &str) -> KeychainResult<()> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| KeychainError::Access(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::Access(e.to_string())),
        }
    }
}

/// Internal helper mirroring [`with_provider`]: route to the global
/// string-secret test provider if installed, otherwise the production OS
/// backend — or the fail-fast sentinel in a test-harness process.
fn with_string_provider<F, R>(f: F) -> R
where
    F: FnOnce(&dyn StringSecretProvider) -> R,
{
    let provider_arc: Option<Arc<dyn StringSecretProvider>> = {
        let guard = GLOBAL_STRING_PROVIDER
            .read()
            .unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(provider) = provider_arc {
        f(provider.as_ref())
    } else if cfg!(test) || running_under_test_harness() {
        f(&TestStringSentinel)
    } else {
        f(&OsStringSecretKeychain)
    }
}

/// Account name for the sharing bearer token stored by the pairing flow.
/// A UTF-8 token (issued by the office server), NOT a 32-byte key — see
/// the string-secret section above.
pub const KEYCHAIN_SHARING_BEARER_ACCOUNT: &str = "sharing-bearer";

/// Read the sharing bearer (or any string secret) from the keychain
/// through the provider seam. `Ok(None)` if no entry exists.
///
/// Read-lenient in a test-harness process WITHOUT an installed mock: this
/// path is hit by incidental consumers (`load_paired_connection` →
/// `load_sharing_bearer` in vocab/model/transcription commands) whose
/// "not paired / no bearer" branch is the legitimate local-fallback
/// outcome, so returning `None` preserves their behavior — while a write
/// or delete without a mock still fails fast via the sentinel.
pub fn get_string_secret(account: &str) -> KeychainResult<Option<String>> {
    let provider_arc: Option<Arc<dyn StringSecretProvider>> = {
        let guard = GLOBAL_STRING_PROVIDER
            .read()
            .unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    match provider_arc {
        Some(provider) => provider.get_string(account),
        // Lenient read: absent bearer reads as "not paired" (see doc).
        None if cfg!(test) || running_under_test_harness() => Ok(None),
        None => OsStringSecretKeychain.get_string(account),
    }
}

/// Store a string secret through the provider seam.
pub fn set_string_secret(account: &str, secret: &str) -> KeychainResult<()> {
    with_string_provider(|p| p.set_string(account, secret))
}

/// Delete a string secret through the provider seam. Idempotent.
pub fn delete_string_secret(account: &str) -> KeychainResult<()> {
    with_string_provider(|p| p.delete_string(account))
}

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

// Serialize tests that mutate the global provider — cargo test runs
// lib tests in parallel by default, which would cause races.
// `into_inner()` recovers from poisoning when a previous test panicked
// while holding the lock.
#[cfg(test)]
static TEST_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_db_key_returns_none_when_absent() {
        let _guard = serial_lock();
        set_test_provider(TestProvider::empty());
        let result = get_db_key().expect("read");
        assert!(
            result.is_none(),
            "expected None on empty keychain, got Some"
        );
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
        assert_eq!(
            first, second,
            "should return the same key on subsequent calls"
        );
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

    #[test]
    fn spawned_thread_sees_global_provider() {
        // Negative-path coverage: a thread spawned during a test must
        // still see the global provider, not fall through to OsKeychain.
        let _guard = serial_lock();
        let key = [88u8; 32];
        set_test_provider(TestProvider::fixed_db_key(key));

        let handle = std::thread::spawn(|| {
            // This runs on a different thread — with thread-local this
            // would have fallen through to OsKeychain and panicked.
            assert!(
                is_test_provider_active(),
                "spawned thread must see the global test provider"
            );
            get_db_key().expect("spawned thread reads mock")
        });

        let result = handle.join().expect("spawned thread didn't panic");
        assert_eq!(result, Some(key));
        clear_test_provider();
    }

    #[test]
    fn worker_outliving_provider_scope_is_isolated() {
        // A worker that outlives the provider scope must not silently
        // fall through to OsKeychain — it should panic via the fail-fast
        // guard instead.
        let _guard = serial_lock();
        let (tx, rx) = std::sync::mpsc::channel();

        set_test_provider(TestProvider::empty());

        let handle = std::thread::spawn(move || {
            // Wait until the provider is cleared
            rx.recv().unwrap();
            // Now the provider is gone — any keychain call should panic
            let result = std::panic::catch_unwind(get_db_key);
            result.is_err() // true = panicked as expected
        });

        clear_test_provider();
        tx.send(()).unwrap();

        let panicked = handle.join().expect("worker joined");
        assert!(
            panicked,
            "worker must panic when reaching OsKeychain without a provider"
        );
    }

    #[test]
    fn concurrent_reads_do_not_race_on_provider() {
        // Multiple threads reading through the global provider concurrently
        // must all see the same mock — no torn reads or lock contention.
        let _guard = serial_lock();
        let key = [44u8; 32];
        set_test_provider(TestProvider::fixed_db_key(key));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let thread_key = key;
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let result = get_db_key().expect("concurrent read");
                    assert_eq!(result, Some(thread_key), "all reads must see mock");
                }
            }));
        }

        for h in handles {
            h.join().expect("concurrent reader didn't panic");
        }
        clear_test_provider();
    }

    #[test]
    fn test_sentinel_panics_without_provider() {
        // Verify that the test-build sentinel fires when no provider is installed.
        // This is the compile-time guarantee that tests can't accidentally
        // reach the real OS keychain.
        let _guard = serial_lock();
        assert!(!is_test_provider_active(), "precondition: no provider");

        let result = std::panic::catch_unwind(get_db_key);
        assert!(
            result.is_err(),
            "get_db_key without a test provider must panic via TestSentinel"
        );
    }
}

#[cfg(test)]
mod test_harness {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Test harness that pairs a mock keychain provider with a temporary
    /// database path. This prevents tests from accidentally touching the
    /// real `medical.db` — the synthetic key only decrypts the temp DB.
    ///
    /// # Example
    /// ```
    /// with_test_db(|db_path, key| {
    ///     let db = Database::open(db_path, Some(key))?;
    ///     // test logic
    ///     Ok(())
    /// });
    /// ```
    pub fn with_test_db<F, R>(f: F) -> R
    where
        F: FnOnce(&PathBuf, [u8; 32]) -> R,
    {
        let _guard = super::serial_lock();

        let temp_dir = TempDir::new().expect("create temp dir");
        let db_path = temp_dir.path().join("test_medical.db");
        let synthetic_key = [0xABu8; 32]; // Deterministic for reproducibility

        let provider = TestProvider::fixed_db_key(synthetic_key);
        set_test_provider(provider);

        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&db_path, synthetic_key)));

        clear_test_provider();
        drop(temp_dir); // Explicit cleanup before lock release

        match result {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_harness_pairs_key_and_db_path() {
            with_test_db(|db_path, key| {
                assert!(db_path.to_string_lossy().contains("test_medical.db"));
                assert_eq!(key, [0xABu8; 32]);
                assert!(is_test_provider_active());
            });
            // Provider cleared after closure
            assert!(!is_test_provider_active());
        }

        #[test]
        fn test_harness_cleans_up_on_panic() {
            let result = std::panic::catch_unwind(|| {
                with_test_db(|_db_path, _key| {
                    panic!("intentional panic");
                });
            });
            assert!(result.is_err());
            // Provider must be cleared even after panic
            assert!(!is_test_provider_active());
        }
    }
}
