//! Token-bucket rate limiter.
//!
//! A simple in-process rate limiter suitable for guarding per-provider
//! API call rates, local-AI request bursts, or any "N requests per
//! minute" budget. Not thread-safe on its own — wrap in a `Mutex` if
//! you need to share it across async tasks.
//!
//! Tokens are refilled continuously (not in discrete per-minute windows)
//! so the limiter behaves like a smooth leaky bucket rather than a
//! fixed-window counter: a burst of `capacity` requests is allowed
//! immediately after a long idle period, then subsequent requests are
//! throttled until tokens accumulate.

use std::time::Instant;

/// A token-bucket rate limiter.
///
/// Constructed with a per-minute capacity; refills continuously at
/// `capacity / 60` tokens per second. `try_acquire` consumes one token
/// and returns `true` on success, `false` when the bucket is empty.
///
/// # Thread safety
///
/// `RateLimiter` is `Send` but not `Sync` — it holds an `Instant` and a
/// `f64` token count with interior mutation on `try_acquire`. Wrap in a
/// `Mutex` to share across tasks.
pub struct RateLimiter {
    capacity: u32,
    tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a limiter that allows `requests_per_minute` requests per
    /// minute.
    ///
    /// The bucket starts full — the first `requests_per_minute` calls to
    /// [`RateLimiter::try_acquire`] will succeed immediately before
    /// throttling kicks in.
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            capacity: requests_per_minute,
            tokens: requests_per_minute as f64,
            refill_rate: requests_per_minute as f64 / 60.0,
            last_refill: Instant::now(),
        }
    }

    /// Attempt to consume one token.
    ///
    /// Returns `true` if the request is allowed (bucket had at least one
    /// token), `false` if the bucket is empty and the caller should
    /// back off. Each call also advances the internal refill clock so
    /// tokens accumulated since the last call are credited.
    pub fn try_acquire(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Add tokens proportional to elapsed time since the last refill.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity as f64);
        self.last_refill = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_capacity() {
        let mut rl = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(rl.try_acquire(), "should be allowed within capacity");
        }
    }

    #[test]
    fn blocks_when_exhausted() {
        let mut rl = RateLimiter::new(3);
        // Drain all tokens
        assert!(rl.try_acquire());
        assert!(rl.try_acquire());
        assert!(rl.try_acquire());
        // Next request must be blocked
        assert!(!rl.try_acquire(), "should be blocked when exhausted");
    }
}
