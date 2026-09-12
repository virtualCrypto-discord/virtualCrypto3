//! A loose, per-user fixed window.
//!
//! The identity is the caller, not the address: a request only reaches a
//! handler once it has passed authentication, so the token's user and the
//! interaction's Discord user are what a limit can be held against.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

    /// Counts one request against `key`, answering whether it is still within
    /// the window's allowance.
    pub fn allow(&self, key: &str) -> bool {
        if self.limit == 0 {
            return true;
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

        entry.1 <= self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_allows_its_limit_and_then_refuses() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));

        assert!(limiter.allow("user:1"));
        assert!(limiter.allow("user:1"));
        assert!(!limiter.allow("user:1"));

        // Another identity has its own allowance.
        assert!(limiter.allow("user:2"));
    }

    #[test]
    fn a_window_that_has_passed_starts_again() {
        let limiter = RateLimiter::new(1, Duration::from_millis(0));

        assert!(limiter.allow("user:1"));
        assert!(limiter.allow("user:1"));
    }
}
