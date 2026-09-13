//! Test-only helpers shared across the crate's `#[cfg(test)]` modules.
//!
//! Two concerns live here:
//!
//! 1. **Keychain isolation** — the `KeychainMockGuard` RAII type installs a
//!    global mock secret provider (see `medical_security::keychain`) for the
//!    duration of a test and removes it on drop, holding a process-wide
//!    serialisation lock so parallel tests never race on the global
//!    provider slot. Keychain-touching tests MUST use it (or the
//!    `with_*` helpers below) instead of calling the keychain directly.
//!
//! 2. **Key/DB pairing** — a mock provider isolates *credentials*, not
//!    filesystem access. The helpers that install a synthetic key either
//!    target the in-memory database or require a temporary DB *path*: the
//!    synthetic key only decrypts a DB created under it, so an accidental
//!    open of the real `medical.db` fails on open rather than silently
//!    succeeding.
//!
//! Nothing here touches recording content, key material, or real user data.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialises tests that mutate the global keychain provider. Cargo runs
/// lib tests in parallel by default; two tests swapping the global
/// provider concurrently would leak each other's mocks.
///
/// `into_inner()` recovers from poisoning when a previous test panicked
/// while holding the lock — the provider is still cleared by the guard's
/// Drop, so a poisoned lock must not wedge every subsequent test.
/// NOTE: this is a SECOND provider lock, separate from medical-security's
/// `serial_test_lock()` (crates/security/src/keychain.rs) — harmless today
/// because the two never share a binary, but silently non-serializing if
/// they ever do; medical-security's is the canonical cross-crate lock.
static PROVIDER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn provider_lock() -> MutexGuard<'static, ()> {
    PROVIDER_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// RAII guard: installs `provider` globally for the lifetime of the guard,
/// holding the provider lock (blocks other mock-installing tests) and
/// clearing the provider on drop — including drop-by-panic.
///
/// # Lock discipline (the deadlock contract)
///
/// The provider lock is a `std::sync::Mutex` — non-reentrant. Two rules,
/// violations of either deadlock the whole parallel suite:
///
/// 1. NEVER hold two guards on the same thread at once (an outer
///    `exclusive_no_provider` clear-check plus an inner re-install
///    self-deadlocks: the inner `lock()` waits forever on a mutex this
///    thread already holds). Scope each acquisition in its own block.
/// 2. NEVER hold a guard across an `.await` in a `#[tokio::test]`: the
///    future can suspend with the std mutex held, starving every other
///    participating test for the suspension's duration. In async tests,
///    bracket the request phases in synchronous install/clear blocks.
///
/// The guard is held for the whole test body otherwise, which is what makes
/// the isolation sound under concurrency: no other participating test can
/// swap the provider mid-flight, and a worker thread that outlives the
/// test's scope fails fast via the keychain sentinel instead of silently
/// falling through to the real OS keychain.
pub(crate) struct KeychainMockGuard {
    _lock: MutexGuard<'static, ()>,
}

impl KeychainMockGuard {
    /// Install a fixed synthetic DB key (`[key; 32]`) as the global mock.
    pub(crate) fn fixed_db_key(key: [u8; 32]) -> Self {
        Self::install(medical_security::keychain::TestProvider::fixed_db_key(key))
    }

    /// Install an empty mock keychain (all lookups return `None`).
    pub(crate) fn empty() -> Self {
        Self::install(medical_security::keychain::TestProvider::empty())
    }

    /// Hold the provider lock WITHOUT installing a provider — for
    /// negative-path tests that assert fail-fast behaviour with no
    /// provider. Serialises against mock-installing tests so the global
    /// "no provider" precondition actually holds for the test's duration.
    pub(crate) fn exclusive_no_provider() -> Self {
        let lock = provider_lock();
        assert!(
            !medical_security::keychain::is_test_provider_active(),
            "precondition violated: a provider is installed while no test holds the lock \
             (leak from a test that bypassed KeychainMockGuard?)"
        );
        Self { _lock: lock }
    }

    fn install(provider: medical_security::keychain::TestProvider) -> Self {
        let _lock = provider_lock();
        // Defensive: a previous test that panicked between install and
        // drop (or a future misuse of set_test_provider without the
        // guard) could leave a stale provider installed. Overwrite it.
        medical_security::keychain::set_test_provider(provider);
        assert!(
            medical_security::keychain::is_test_provider_active(),
            "mock provider must be active after install"
        );
        Self { _lock }
    }
}

impl Drop for KeychainMockGuard {
    fn drop(&mut self) {
        medical_security::keychain::clear_test_provider();
    }
}

/// RAII guard: installs a global string-secret mock provider (the
/// sharing-bearer seam, `medical_security::keychain::set_test_string_provider`)
/// for the guard's lifetime and clears it on drop — including drop-by-panic.
///
/// Unlike [`KeychainMockGuard`] this takes NO provider lock: the sharing
/// command tests are async and the install must be legal across `.await`s,
/// and a std-mutex guard held across an await violates the deadlock
/// contract above. Instead, callers MUST already hold
/// `commands::sharing::test_app_data_guard()` (or otherwise serialize) —
/// every current user does, which serializes string-provider installs
/// against each other.
pub(crate) struct StringSecretMockGuard;

impl StringSecretMockGuard {
    /// Install an empty string-secret keychain (bearer absent).
    pub(crate) fn empty() -> Self {
        medical_security::keychain::set_test_string_provider(
            medical_security::keychain::TestStringProvider::empty(),
        );
        assert!(
            medical_security::keychain::is_test_string_provider_active(),
            "string mock provider must be active after install"
        );
        Self
    }

    /// Install a string-secret keychain pre-loaded with `account -> secret`.
    /// Use synthetic fixture tokens only — never real key material.
    pub(crate) fn with_secrets(secrets: std::collections::HashMap<String, String>) -> Self {
        medical_security::keychain::set_test_string_provider(
            medical_security::keychain::TestStringProvider::with_secrets(secrets),
        );
        assert!(
            medical_security::keychain::is_test_string_provider_active(),
            "string mock provider must be active after install"
        );
        Self
    }
}

impl Drop for StringSecretMockGuard {
    fn drop(&mut self) {
        medical_security::keychain::clear_test_string_provider();
    }
}

/// Assert that `db_path` lives inside a temporary directory — the pairing
/// requirement for file-backed databases under a synthetic key (see the
/// module docs). Panics (failing the test) on any path that could be the
/// real application database.
pub(crate) fn assert_temp_db_path(db_path: &Path) {
    assert!(
        db_path.is_absolute(),
        "test DB path must be absolute, got relative path (refusing to guess what it touches)"
    );
    let s = db_path.to_string_lossy();
    let in_temp = s.starts_with("/tmp/")
        || s.starts_with("/var/folders/") // macOS tempdirs
        || s.starts_with("/private/tmp/")
        || s.starts_with("/private/var/folders/")
        || s.starts_with("/dev/shm/")
        || std::env::temp_dir().eq(db_path.parent().unwrap_or(db_path));
    assert!(
        in_temp,
        "test DB path must live in a tempdir (got a path outside known temp locations — \
         refusing to run a synthetic-key test against it)"
    );
    // Belt-and-braces: never the production DB file name at its real root.
    assert!(
        !db_path.ends_with("medical.db")
            || s.starts_with("/tmp/")
            || s.starts_with("/var/folders/")
            || s.starts_with("/private/tmp/"),
        "test DB path must not be a non-temp 'medical.db'"
    );
}

/// Run `f` with a fixed synthetic DB key installed AND an in-memory
/// database scope — the standard shape for keychain-touching unit tests
/// that don't need a file-backed DB.
pub(crate) fn with_mock_key_and_memory_db<F, R>(synthetic_key: [u8; 32], f: F) -> R
where
    F: FnOnce(&medical_db::Database) -> R,
{
    let _guard = KeychainMockGuard::fixed_db_key(synthetic_key);
    let db = medical_db::Database::open_in_memory().expect("open in-memory test db");
    f(&db)
}

/// Run `f` with a fixed synthetic DB key installed AND a temporary
/// file-backed database. The pairing requirement is enforced: `db_path`
/// must be inside a tempdir (see [`assert_temp_db_path`]) and the database
/// is created under the synthetic key, so the real `medical.db` — which
/// was keyed with the real OS keychain key — could never open under this
/// mock even if a path confusion regressed.
pub(crate) fn with_mock_key_and_temp_db<F, R>(synthetic_key: [u8; 32], db_path: &Path, f: F) -> R
where
    F: FnOnce(&medical_db::Database, &Path) -> R,
{
    assert_temp_db_path(db_path);
    let _guard = KeychainMockGuard::fixed_db_key(synthetic_key);
    let db = medical_db::Database::open(db_path, Some(synthetic_key)).expect("open temp test db");
    f(&db, db_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_guard_installs_and_clears_provider() {
        let _guard = KeychainMockGuard::fixed_db_key([0x11u8; 32]);
        assert!(medical_security::keychain::is_test_provider_active());
        drop(_guard);
        // Deterministic clear-check: acquire the provider lock and assert
        // no provider remains (asserting outside the lock could race a
        // parallel test's install).
        let _ex = KeychainMockGuard::exclusive_no_provider();
    }

    #[test]
    fn mock_guard_clears_provider_on_panic() {
        let result = std::panic::catch_unwind(|| {
            let _guard = KeychainMockGuard::empty();
            panic!("simulated test failure");
        });
        assert!(result.is_err());
        let _ex = KeychainMockGuard::exclusive_no_provider();
    }

    #[test]
    fn temp_db_path_accepts_tempdirs_and_rejects_others() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ok = tmp.path().join("test.db");
        assert_temp_db_path(&ok); // must not panic
        assert_temp_db_path(std::path::Path::new("/tmp/anything.db"));

        for rejected in [
            "/Users/shared/medical.db",
            "/Library/Application Support/rust-medical-assistant/medical.db",
        ] {
            let result =
                std::panic::catch_unwind(|| assert_temp_db_path(std::path::Path::new(rejected)));
            assert!(
                result.is_err(),
                "non-temp path must be rejected: {rejected}"
            );
        }

        // Relative path — rejected before anything can resolve it.
        let rel =
            std::panic::catch_unwind(|| assert_temp_db_path(std::path::Path::new("medical.db")));
        assert!(rel.is_err(), "relative path must be rejected");
    }

    #[test]
    fn temp_db_helper_creates_db_under_synthetic_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("pairing.db");
        let key = [0x42u8; 32];
        with_mock_key_and_temp_db(key, &db_path, |db, path| {
            let conn = db.conn().expect("conn");
            let v: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("query");
            assert_eq!(v, 1);
            assert_eq!(path, &db_path);
            assert!(
                medical_security::keychain::is_test_provider_active(),
                "provider must be installed for the whole closure scope"
            );
        });
        // Clear-check under the lock (an unlocked assert could race a
        // parallel test's install). Scoped: the guard MUST drop before the
        // reopen guard below re-acquires the same non-reentrant
        // std::sync::Mutex on this thread — nesting the two acquisitions
        // is a self-deadlock (the bug this restructure fixed).
        {
            let _ex = KeychainMockGuard::exclusive_no_provider();
        }
        // The temp DB is readable back under the same synthetic key.
        let reopened_ok = {
            let _guard = KeychainMockGuard::fixed_db_key(key);
            medical_db::Database::open(&db_path, Some(key)).is_ok()
        };
        assert!(reopened_ok, "temp DB must reopen under the same key");
    }

    #[test]
    fn file_db_under_a_different_key_fails_on_open() {
        // THE pairing guarantee, proven: a DB created under synthetic key
        // A cannot be opened under synthetic key B — so if a future test
        // regresses to pointing at the real DB path with a synthetic key,
        // it fails on open, loudly, instead of silently reading data.
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("cross-key.db");
        let key_a = [0x01u8; 32];
        with_mock_key_and_temp_db(key_a, &db_path, |_, _| {});

        let _guard = KeychainMockGuard::fixed_db_key([0x02u8; 32]);
        let reopened = medical_db::Database::open(&db_path, Some([0x02u8; 32]));
        assert!(
            reopened.is_err(),
            "opening a temp DB under the WRONG key must fail on open"
        );
    }

    #[test]
    fn memory_db_helper_scopes_provider() {
        with_mock_key_and_memory_db([0x33u8; 32], |db| {
            let conn = db.conn().expect("conn");
            let v: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).expect("query");
            assert_eq!(v, 1);
            assert!(medical_security::keychain::is_test_provider_active());
        });
        // Clear-check under the lock (an unlocked assert could race a
        // parallel test's install).
        let _ex = KeychainMockGuard::exclusive_no_provider();
    }

    /// Negative-path companion: with the provider lock held and NO
    /// provider installed (serialized against mock-holding tests), a
    /// keychain read must panic via the fail-fast sentinel — this is the
    /// runtime-detection guarantee on which "tests never reach the real
    /// keychain" rests, expressed as a test rather than inspection.
    #[test]
    fn keychain_read_without_provider_fails_fast() {
        let _guard = KeychainMockGuard::exclusive_no_provider();
        let panicked =
            std::panic::catch_unwind(|| medical_security::keychain::get_db_key().is_ok()).is_err();
        assert!(
            panicked,
            "keychain read with no provider must panic via the sentinel, not touch the OS keychain"
        );
    }

    #[test]
    fn keychain_read_on_spawned_thread_uses_the_mock() {
        // Concurrency scope: work dispatched to another thread during the
        // guard's scope (the shape of every spawn_blocking call in the
        // commands) must resolve through the global mock, never the OS
        // keychain.
        let key = [0x44u8; 32];
        let _guard = KeychainMockGuard::fixed_db_key(key);
        let handle = std::thread::spawn(|| {
            assert!(medical_security::keychain::is_test_provider_active());
            medical_security::keychain::get_db_key().expect("read via mock")
        });
        assert_eq!(handle.join().expect("thread"), Some(key));
    }
}
