use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

/// How many requests a key may make, and over what window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed in a window.
    pub max_requests: u32,
    /// The length of a window.
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 10,
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
struct Record {
    window_start: Instant,
    count: u32,
}

/// An in-memory fixed-window rate limiter, keyed by whatever the caller
/// counts against: a user id, an API key, a host or a tenant.
///
/// ```
/// use abnegate_http::{RateLimitConfig, RateLimiter};
/// use std::time::Duration;
///
/// let limiter: RateLimiter<String> = RateLimiter::new(RateLimitConfig {
///     max_requests: 1,
///     window: Duration::from_secs(60),
/// });
///
/// assert!(limiter.check_rate_limit("tenant-a".to_string()).0);
/// assert!(!limiter.check_rate_limit("tenant-a".to_string()).0);
/// assert!(limiter.check_rate_limit("tenant-b".to_string()).0);
/// ```
#[derive(Debug, Clone)]
pub struct RateLimiter<K: Eq + Hash + Clone + Send + Sync> {
    config: RateLimitConfig,
    records: Arc<DashMap<K, Record>>,
}

impl<K: Eq + Hash + Clone + Send + Sync> RateLimiter<K> {
    /// Create a rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            records: Arc::new(DashMap::new()),
        }
    }

    /// The configuration this limiter counts against.
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Whether a request for `key` should be allowed.
    ///
    /// Returns whether the request is allowed, how many requests remain in the
    /// window, and when the window resets.
    pub fn check_rate_limit(&self, key: K) -> (bool, u32, Instant) {
        let now = Instant::now();

        let mut entry = self.records.entry(key).or_insert_with(|| Record {
            window_start: now,
            count: 0,
        });
        let record = entry.value_mut();

        if now.duration_since(record.window_start) >= self.config.window {
            record.window_start = now;
            record.count = 0;
        }

        // Incrementing before the check is what makes this safe under
        // concurrency: two requests that both read a count one below the limit
        // would otherwise both be allowed.
        record.count = record.count.saturating_add(1);

        let reset_at = record.window_start + self.config.window;
        if record.count > self.config.max_requests {
            record.count = record.count.saturating_sub(1);
            return (false, 0, reset_at);
        }

        (
            true,
            self.config.max_requests.saturating_sub(record.count),
            reset_at,
        )
    }

    /// Drop the keys whose windows are long past, so a limiter that has seen
    /// many keys does not hold them all forever.
    pub fn cleanup(&self) {
        let now = Instant::now();
        let window = self.config.window;

        self.records
            .retain(|_, record| now.duration_since(record.window_start) < window * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use uuid::Uuid;

    #[test]
    fn rate_limit_allows_requests_within_limit() {
        let limiter: RateLimiter<Uuid> = RateLimiter::new(RateLimitConfig {
            max_requests: 3,
            window: Duration::from_secs(60),
        });
        let user = Uuid::new_v4();

        let (allowed, remaining, _) = limiter.check_rate_limit(user);
        assert!(allowed);
        assert_eq!(remaining, 2);

        let (allowed, remaining, _) = limiter.check_rate_limit(user);
        assert!(allowed);
        assert_eq!(remaining, 1);

        let (allowed, remaining, _) = limiter.check_rate_limit(user);
        assert!(allowed);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn rate_limit_blocks_requests_over_limit() {
        let limiter: RateLimiter<Uuid> = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
        });
        let user = Uuid::new_v4();

        assert!(limiter.check_rate_limit(user).0);
        assert!(limiter.check_rate_limit(user).0);

        let (allowed, remaining, _) = limiter.check_rate_limit(user);
        assert!(!allowed);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn rate_limit_resets_after_window() {
        let limiter: RateLimiter<Uuid> = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window: Duration::from_millis(100),
        });
        let user = Uuid::new_v4();

        assert!(limiter.check_rate_limit(user).0);
        assert!(limiter.check_rate_limit(user).0);
        assert!(!limiter.check_rate_limit(user).0);

        sleep(Duration::from_millis(110));

        let (allowed, remaining, _) = limiter.check_rate_limit(user);
        assert!(allowed);
        assert_eq!(remaining, 1);
    }

    #[test]
    fn rate_limit_different_users_independent() {
        let limiter: RateLimiter<Uuid> = RateLimiter::new(RateLimitConfig {
            max_requests: 1,
            window: Duration::from_secs(60),
        });
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();

        assert!(limiter.check_rate_limit(first).0);
        assert!(!limiter.check_rate_limit(first).0);

        assert!(limiter.check_rate_limit(second).0);
    }

    #[test]
    fn rate_limit_cleanup_removes_old_entries() {
        let limiter: RateLimiter<Uuid> = RateLimiter::new(RateLimitConfig {
            max_requests: 10,
            window: Duration::from_millis(50),
        });

        for _ in 0..5 {
            limiter.check_rate_limit(Uuid::new_v4());
        }
        assert_eq!(limiter.records.len(), 5);

        sleep(Duration::from_millis(150));
        limiter.cleanup();
        assert_eq!(limiter.records.len(), 0);

        assert!(limiter.check_rate_limit(Uuid::new_v4()).0);
    }

    #[test]
    fn rate_limit_reset_timestamp() {
        let config = RateLimitConfig {
            max_requests: 1,
            window: Duration::from_secs(60),
        };
        let limiter: RateLimiter<Uuid> = RateLimiter::new(config);

        let start = Instant::now();
        let (_, _, reset_at) = limiter.check_rate_limit(Uuid::new_v4());

        let expected = start + config.window;
        let difference = if reset_at > expected {
            reset_at - expected
        } else {
            expected - reset_at
        };
        assert!(difference < Duration::from_millis(10), "{difference:?}");
    }

    #[test]
    fn a_limiter_counts_against_whatever_key_it_was_given() {
        let limiter: RateLimiter<&'static str> = RateLimiter::new(RateLimitConfig {
            max_requests: 1,
            window: Duration::from_secs(60),
        });

        assert!(limiter.check_rate_limit("api.example").0);
        assert!(!limiter.check_rate_limit("api.example").0);
        assert!(limiter.check_rate_limit("other.example").0);
    }

    #[test]
    fn the_configuration_is_readable_back() {
        let config = RateLimitConfig::default();
        let limiter: RateLimiter<Uuid> = RateLimiter::new(config);

        assert_eq!(*limiter.config(), config);
    }
}
