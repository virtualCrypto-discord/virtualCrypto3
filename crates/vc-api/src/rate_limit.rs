//! A loose, per-user fixed window.
//!
//! The identity is the caller, not the address: a request only reaches a
//! handler once it has passed authentication, so the token's user and the
//! interaction's Discord user are what a limit can be held against.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderValue;

/// How many requests an identity may make before the window resets, and how long
/// the window is.
pub const DEFAULT_LIMIT: u32 = 120;
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

/// Past this many identities, expired windows are swept; without it the table
/// would only ever grow.
const SWEEP_AT: usize = 10_000;

pub struct RateLimiter {
    limit: u32,
    window: Duration,
    hits: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one request against `key`.
    ///
    /// `Err` is the wait left in the window that refused, which is the number a
    /// `Retry-After` is written from: a fixed window resets when the one it is
    /// counting from is over, not when the rejecting request happened.
    pub fn allow(&self, key: &str) -> Result<(), Duration> {
        if self.limit == 0 {
            return Ok(());
        }

        let mut hits = self.hits.lock().expect("the limiter is not poisoned");

        if hits.len() >= SWEEP_AT {
            let window = self.window;
            hits.retain(|_, (started, _)| started.elapsed() < window);
        }

        let now = Instant::now();
        let entry = hits.entry(key.to_string()).or_insert((now, 0));

        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }

        entry.1 += 1;

        if entry.1 <= self.limit {
            Ok(())
        } else {
            Err(self.window.saturating_sub(now.duration_since(entry.0)))
        }
    }
}

/// What a refusal tells a client to wait for, in the seconds `Retry-After` is
/// spelled in.
///
/// Rounded up and floored at one: the window has not reset when the request was
/// refused, so the wait is never already over, and a whole number of seconds is
/// what the header carries.
pub fn retry_after(remaining: Duration) -> HeaderValue {
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0);

    HeaderValue::from_str(&seconds.max(1).to_string())
        .expect("a count of seconds is a valid header value")
}

/// The handshake's allowance: per requester, in three windows.
///
/// Three of the same limiter rather than one, because the windows answer different
/// questions — the short one is a retry storm, the hour is somebody hammering, and
/// the day bounds how much of this service's time one requester can spend. The
/// Elixir writes them as three `Hammer.check_rate/3` calls in the same order.
pub struct VerificationLimiter {
    short: RateLimiter,
    hour: RateLimiter,
    day: RateLimiter,
}

/// Which window refused, and therefore what a client is told to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooSoon {
    Seconds3,
    Hour,
    Day,
}

impl VerificationLimiter {
    pub fn new() -> Self {
        Self {
            short: RateLimiter::new(1, Duration::from_secs(3)),
            hour: RateLimiter::new(20, Duration::from_secs(60 * 60)),
            day: RateLimiter::new(50, Duration::from_secs(24 * 60 * 60)),
        }
    }

    /// `None` when the handshake may be attempted.
    ///
    /// Otherwise the window that refused — which one it is says what a client is
    /// being asked to wait for — and how long of that wait is left.
    ///
    /// The windows are asked in the Elixir's order, and each has its own key, so
    /// that a requester's hour and their day are counted separately rather than one
    /// window's answer counting for all three.
    pub fn refuse(&self, requester: &str) -> Option<(TooSoon, Duration)> {
        if let Err(remaining) = self.short.allow(&format!("verification_sec:{requester}")) {
            return Some((TooSoon::Seconds3, remaining));
        }

        if let Err(remaining) = self.hour.allow(&format!("verification_hour:{requester}")) {
            return Some((TooSoon::Hour, remaining));
        }

        if let Err(remaining) = self.day.allow(&format!("verification_day:{requester}")) {
            return Some((TooSoon::Day, remaining));
        }

        None
    }
}

impl Default for VerificationLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_allows_its_limit_and_then_refuses() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));

        assert!(limiter.allow("user:1").is_ok());
        assert!(limiter.allow("user:1").is_ok());

        // And the refusal says how much of the window is left to sit out: a wait
        // that has not already passed, and no longer than the window itself.
        let remaining = limiter
            .allow("user:1")
            .expect_err("the third is over the limit");
        assert!(remaining > Duration::ZERO && remaining <= Duration::from_secs(60));

        // Another identity has its own allowance.
        assert!(limiter.allow("user:2").is_ok());
    }

    #[test]
    fn a_window_that_has_passed_starts_again() {
        let limiter = RateLimiter::new(1, Duration::from_millis(0));

        assert!(limiter.allow("user:1").is_ok());
        assert!(limiter.allow("user:1").is_ok());
    }

    /// A refusal is never already over: a client told to wait a second has a
    /// second to wait, whatever the window's own arithmetic came to.
    #[test]
    fn a_refusal_is_never_zero_seconds() {
        assert_eq!(retry_after(Duration::ZERO), "1");
        assert_eq!(retry_after(Duration::from_millis(1)), "1");
        assert_eq!(retry_after(Duration::from_millis(1_001)), "2");
        assert_eq!(retry_after(Duration::from_secs(59)), "59");
    }

    /// The first handshake of a requester is allowed and the second is not: the
    /// short window is one per three seconds, so nothing has to be waited for to
    /// see it.
    #[test]
    fn a_requester_may_ask_once_and_then_not_yet() {
        let limiter = VerificationLimiter::new();

        assert_eq!(limiter.refuse("someone"), None);
        assert!(
            matches!(limiter.refuse("someone"), Some((TooSoon::Seconds3, _))),
            "the short window is the one that refuses"
        );
    }

    /// Per requester, not globally: somebody else's handshake is their own.
    #[test]
    fn one_requesters_refusal_is_not_anothers() {
        let limiter = VerificationLimiter::new();

        assert_eq!(limiter.refuse("one"), None);
        assert_eq!(limiter.refuse("two"), None);
    }

    /// Which window refused, and how much of it is left: a client told to wait
    /// for the three-second window is told a wait that has not already run out.
    #[test]
    fn a_refused_handshake_says_how_long_is_left() {
        let limiter = VerificationLimiter::new();

        limiter.refuse("someone");
        let (which, remaining) = limiter.refuse("someone").expect("the second is refused");

        assert_eq!(which, TooSoon::Seconds3);
        assert!(remaining > Duration::ZERO && remaining <= Duration::from_secs(3));
    }
}
