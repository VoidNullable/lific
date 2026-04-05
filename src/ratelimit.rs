use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Simple in-memory rate limiter.
/// Tracks attempts per key (e.g. username or IP) within a sliding window.
#[derive(Debug)]
pub struct RateLimiter {
    /// (key -> list of attempt timestamps)
    attempts: Mutex<HashMap<String, Vec<Instant>>>,
    /// Maximum attempts allowed within the window.
    max_attempts: usize,
    /// Window duration.
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window,
        }
    }

    /// Record an attempt for the given key.
    /// Returns `true` if the attempt is allowed, `false` if rate-limited.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap();

        let entry = map.entry(key.to_string()).or_default();

        // Prune expired entries
        entry.retain(|t| now.duration_since(*t) < self.window);

        if entry.len() >= self.max_attempts {
            return false;
        }

        entry.push(now);
        true
    }

    /// Record a failed attempt without checking first (for tracking after auth failure).
    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap();
        let entry = map.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        entry.push(now);
    }

    /// How many seconds until the oldest attempt in the window expires.
    pub fn retry_after(&self, key: &str) -> u64 {
        let now = Instant::now();
        let map = self.attempts.lock().unwrap();
        match map.get(key) {
            Some(entries) if !entries.is_empty() => {
                let oldest = entries[0];
                let elapsed = now.duration_since(oldest);
                if elapsed < self.window {
                    (self.window - elapsed).as_secs() + 1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_under_limit() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
    }

    #[test]
    fn blocks_over_limit() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
        assert!(!rl.check("user1")); // blocked
    }

    #[test]
    fn different_keys_independent() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user2")); // different key, still allowed
        assert!(!rl.check("user1")); // same key, blocked
    }

    #[test]
    fn retry_after_nonzero_when_limited() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        rl.check("user1");
        assert!(rl.retry_after("user1") > 0);
    }
}
