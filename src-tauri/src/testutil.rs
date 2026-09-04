//! Test-only helpers shared across the crate's `#[cfg(test)]` modules.
//!
//! Nothing here touches recording content — the keychain guard shuttles a
//! file path and an `io::Result` between threads, nothing else.

/// How long to wait on an OS-keychain operation before declaring it
/// unresponsive. A healthy keychain answers in milliseconds; 5 s only
/// elapses when securityd is blocked on a user-facing prompt.
const KEYCHAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a keychain-dependent closure with a hang guard, returning `None`
/// when the OS keychain is unresponsive.
///
/// macOS securityd can block keychain access indefinitely — most commonly
/// by putting up an access prompt the test harness cannot see or dismiss
/// (unsigned test binaries, a keychain that locked between runs). The test
/// runner has no per-test timeout, so one blocked call hangs the whole
/// `cargo test` binary; this actually happened on the workspace gate
/// (2026-09-03: three lib tests stuck ~35 min inside
/// `keychain::get_or_create_secret`).
///
/// The closure runs on a dedicated worker thread; on timeout the worker is
/// left blocked and dies with the process when the harness exits. Callers
/// should treat `None` like the existing "keychain unavailable" environment
/// (headless CI) and assert the weaker invariant instead of hanging.
///
/// A worker that panics propagates the panic — the guard must never
/// swallow real test bugs.
pub(crate) fn with_keychain_guard<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(KEYCHAIN_WAIT) {
        Ok(value) => Some(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            eprintln!(
                "skipping keychain assertions: the OS keychain did not answer within {:?} \
                 (an access prompt may be waiting) — run the test again after granting access",
                KEYCHAIN_WAIT
            );
            None
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("keychain guard worker panicked before reporting a result");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_returns_the_closure_result() {
        assert_eq!(with_keychain_guard(|| 41 + 1), Some(42));
    }

    #[test]
    fn guard_times_out_on_a_blocked_closure() {
        // A closure that never finishing simulates a securityd prompt:
        // the guard must give up instead of hanging the harness.
        let started = std::time::Instant::now();
        let none = with_keychain_guard(|| -> u8 {
            std::thread::sleep(std::time::Duration::from_secs(60));
            0
        });
        assert!(none.is_none(), "blocked closure must yield None");
        assert!(
            started.elapsed() >= KEYCHAIN_WAIT,
            "guard returned before its own timeout"
        );
    }

    #[test]
    fn guard_propagates_worker_panic() {
        let result = std::panic::catch_unwind(|| {
            let _ = with_keychain_guard(|| -> u8 { panic!("worker bug") });
        });
        assert!(
            result.is_err(),
            "a panicking worker must panic the caller, not read as a skip"
        );
    }
}
