//! Pairing service -- one-shot 6-digit enrollment codes that exchange for
//! long-lived per-client tokens.
//!
//! ## Flow
//!
//! 1. Server admin calls [`PairingState::issue_code`] to generate a
//!    time-limited 6-digit code (displayed in the UI or encoded as a QR URL).
//! 2. Client submits the code to `/pair/enroll` along with a human-readable
//!    label (e.g. `"clinic-laptop"`).
//! 3. [`PairingState::enroll`] validates the code (exact match, not expired,
//!    not already consumed), then calls [`TokenStore::issue`] to generate a
//!    long-lived bearer token.
//! 4. The code is consumed (one-shot). The token is returned to the client
//!    for all subsequent requests.
//!
//! Only one code is active at a time. Issuing a new code replaces the
//! previous one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::sync::Mutex;

use crate::token_store::{TokenStore, TokenStoreError};

/// Errors that can occur during the pairing flow.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// The submitted code doesn't match the active code, or the code was
    /// already consumed.
    #[error("invalid or already-used code")]
    InvalidCode,
    /// The active code's TTL has elapsed.
    #[error("code expired")]
    Expired,
    /// Too many failed enrollment attempts. The code is invalidated; the
    /// admin must issue a new one. Prevents brute-force of the 6-digit code.
    #[error("too many failed attempts; issue a new code")]
    LockedOut,
    /// Underlying token store error.
    #[error("token store: {0}")]
    Store(#[from] TokenStoreError),
}

/// Convenience alias for `Result<T, PairingError>`.
pub type Result<T> = std::result::Result<T, PairingError>;

const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);

/// Maximum failed enrollment attempts before the code is locked out and must
/// be re-issued. Caps brute-force of the 6-digit (1M) code space: at this
/// threshold an attacker has a ~0.0005% chance of guessing before lockout.
const MAX_FAILED_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone)]
struct ActiveCode {
    code: String,
    issued_at: Instant,
    /// Failed enrollment attempts since issue. Resets on `issue_code`.
    failed_attempts: u32,
}

/// Manages the lifecycle of pairing codes and their exchange for tokens.
///
/// Thread-safe (internal `Mutex`); designed to be held behind an `Arc` and
/// shared between the orchestrator and the pairing HTTP router.
pub struct PairingState {
    store: Arc<TokenStore>,
    active: Mutex<Option<ActiveCode>>,
    ttl: Duration,
}

impl PairingState {
    /// Create a new pairing state backed by the given token store.
    ///
    /// Uses a default TTL of 10 minutes for issued codes.
    pub fn new(store: Arc<TokenStore>) -> Self {
        Self {
            store,
            active: Mutex::new(None),
            ttl: DEFAULT_TTL,
        }
    }

    /// Override the default code TTL (builder pattern).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Issue (or rotate) the 6-digit code.
    pub async fn issue_code(&self) -> String {
        let code = generate_code();
        let mut guard = self.active.lock().await;
        *guard = Some(ActiveCode {
            code: code.clone(),
            issued_at: Instant::now(),
            failed_attempts: 0,
        });
        code
    }

    /// Show the current active code (or `None` if none / expired).
    pub async fn current_code(&self) -> Option<String> {
        let guard = self.active.lock().await;
        guard.as_ref().and_then(|a| {
            if a.issued_at.elapsed() <= self.ttl {
                Some(a.code.clone())
            } else {
                None
            }
        })
    }

    /// Consume a code and issue a long-lived token. One-shot semantics.
    ///
    /// Tracks failed attempts and locks out after [`MAX_FAILED_ATTEMPTS`],
    /// invalidating the code so an attacker on the LAN can't brute-force the
    /// 6-digit space. The admin must issue a new code after a lockout.
    pub async fn enroll(&self, submitted: &str, label: &str) -> Result<String> {
        let mut guard = self.active.lock().await;
        let active = guard.as_mut().ok_or(PairingError::InvalidCode)?;
        if active.issued_at.elapsed() > self.ttl {
            *guard = None;
            return Err(PairingError::Expired);
        }
        if active.code != submitted {
            active.failed_attempts = active.failed_attempts.saturating_add(1);
            if active.failed_attempts >= MAX_FAILED_ATTEMPTS {
                // Lock out: invalidate the code entirely.
                tracing::warn!(
                    attempts = active.failed_attempts,
                    "pairing locked out after too many failed attempts; code invalidated"
                );
                *guard = None;
                return Err(PairingError::LockedOut);
            }
            return Err(PairingError::InvalidCode);
        }
        let issued = self.store.issue(label).map_err(PairingError::from)?;
        *guard = None; // one-shot
        Ok(issued.token)
    }
}

/// Generate a cryptographically random 6-digit zero-padded code (000000-999999).
///
/// Uses `rand::thread_rng()` for entropy. The output space is 1 million
/// values -- sufficient for a one-shot, time-limited pairing code that a
/// human types in.
pub fn generate_code() -> String {
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generate_code_is_six_digits() {
        let code = generate_code();
        assert_eq!(code.len(), 6, "got {:?}", code);
        assert!(
            code.chars().all(|c| c.is_ascii_digit()),
            "non-digit in {:?}",
            code
        );
    }

    #[test]
    fn generate_code_produces_distinct_outputs() {
        // 100 draws from a 1-million space → birthday collisions are astronomically
        // rare. A duplicate rate above 5% indicates a broken RNG.
        let mut seen = HashSet::new();
        for _ in 0..100 {
            seen.insert(generate_code());
        }
        assert!(
            seen.len() >= 95,
            "RNG looks weak: only {} unique codes out of 100",
            seen.len()
        );
    }

    #[test]
    fn generate_code_covers_the_full_digit_range() {
        // 1000 draws should hit every first-digit at least once if uniform.
        // Use a small set of leading digits as a sanity check.
        let mut first_digits = HashSet::new();
        for _ in 0..1000 {
            if let Some(c) = generate_code().chars().next() {
                first_digits.insert(c);
            }
        }
        assert!(
            first_digits.contains(&'0'),
            "1000 draws produced no leading-0 code — RNG distribution is suspect"
        );
        assert!(
            first_digits.contains(&'9'),
            "1000 draws produced no leading-9 code — RNG distribution is suspect"
        );
    }

    /// Open a TokenStore in a tempdir for lockout tests.
    fn fresh_pairing() -> PairingState {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        // Leak the TempDir so it outlives the test — these are short unit tests.
        std::mem::forget(dir);
        let store = Arc::new(TokenStore::open(&path, &[7u8; 32]).expect("open store"));
        PairingState::new(store)
    }

    #[tokio::test]
    async fn enroll_locks_out_after_max_failed_attempts() {
        let pairing = fresh_pairing();
        let _code = pairing.issue_code().await;
        // MAX_FAILED_ATTEMPTS - 1 wrong codes return InvalidCode...
        for _ in 0..(MAX_FAILED_ATTEMPTS - 1) {
            assert!(matches!(
                pairing.enroll("000000", "attacker").await,
                Err(PairingError::InvalidCode)
            ));
        }
        // ...the MAX_FAILED_ATTEMPTS-th wrong code locks out and invalidates.
        let last = pairing.enroll("000000", "attacker").await;
        assert!(matches!(last, Err(PairingError::LockedOut)));
        // Code is now invalidated — even the real code can't enroll.
        let real = pairing.current_code().await;
        assert!(real.is_none(), "code must be invalidated after lockout");
    }

    #[tokio::test]
    async fn enroll_resets_attempt_count_on_new_code() {
        let pairing = fresh_pairing();
        let _code = pairing.issue_code().await;
        // Burn (MAX_FAILED_ATTEMPTS - 1) attempts.
        for _ in 0..(MAX_FAILED_ATTEMPTS - 1) {
            let _ = pairing.enroll("000000", "attacker").await;
        }
        // Issue a new code — attempt count resets.
        let code = pairing.issue_code().await;
        // Now a correct enrollment succeeds despite the prior failures.
        let token = pairing.enroll(&code, "client").await;
        assert!(token.is_ok(), "new code must reset the attempt counter");
    }

    #[tokio::test]
    async fn enroll_correct_code_within_limit_succeeds() {
        let pairing = fresh_pairing();
        let code = pairing.issue_code().await;
        // A couple of wrong guesses, then the right one.
        let _ = pairing.enroll("000000", "x").await;
        let _ = pairing.enroll("111111", "x").await;
        let token = pairing.enroll(&code, "client").await;
        assert!(token.is_ok());
    }
}
