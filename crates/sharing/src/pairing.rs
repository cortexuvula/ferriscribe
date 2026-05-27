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
    /// Underlying token store error.
    #[error("token store: {0}")]
    Store(#[from] TokenStoreError),
}

/// Convenience alias for `Result<T, PairingError>`.
pub type Result<T> = std::result::Result<T, PairingError>;

const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
struct ActiveCode {
    code: String,
    issued_at: Instant,
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
        *guard = Some(ActiveCode { code: code.clone(), issued_at: Instant::now() });
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
    pub async fn enroll(&self, submitted: &str, label: &str) -> Result<String> {
        let mut guard = self.active.lock().await;
        let active = guard.as_ref().ok_or(PairingError::InvalidCode)?.clone();
        if active.issued_at.elapsed() > self.ttl {
            *guard = None;
            return Err(PairingError::Expired);
        }
        if active.code != submitted {
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
}
