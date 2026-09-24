mod config;
mod decision;
mod record;

use crate::limit::record::Record;
use dashmap::DashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Instant;

pub use crate::limit::config::RateLimitConfig;
pub use crate::limit::decision::Decision;

/// How many windows a key's record outlives its own before
/// [`RateLimiter::cleanup`] drops it.
const RETAINED_WINDOWS: u32 = 2;

/// An in-memory fixed-window rate limiter, keyed by whatever the caller
/// counts against: a user id, an API key, a host or a tenant.
///
/// ```
/// use abnegate_http::{RateLimitConfig, RateLimiter};
/// use std::time::Duration;
///
/// let limiter: RateLimiter<String> = RateLimiter::new(RateLimitConfig {
///     limit: 1,
///     window: Duration::from_secs(60),
/// });
///
/// assert!(limiter.check_rate_limit("tenant-a".to_string()).allowed);
/// assert!(!limiter.check_rate_limit("tenant-a".to_string()).allowed);
/// assert!(limiter.check_rate_limit("tenant-b".to_string()).allowed);
/// ```
pub struct RateLimiter<K> {
    config: RateLimitConfig,
    records: Arc<DashMap<K, Record>>,
}

impl<K: Eq + Hash> RateLimiter<K> {
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

    /// Count a request for `key` and decide whether it may proceed.
    pub fn check_rate_limit(&self, key: K) -> Decision {
        self.check_rate_limit_at(key, Instant::now())
    }

    /// Drop the keys whose windows are long past, so a limiter that has seen
    /// many keys does not hold them all forever.
    pub fn cleanup(&self) {
        self.cleanup_at(Instant::now());
    }

    fn check_rate_limit_at(&self, key: K, now: Instant) -> Decision {
        let window = self.config.window;
        let mut entry = self.records.entry(key).or_insert_with(|| Record::new(now));
        let record = entry.value_mut();

        if now.saturating_duration_since(record.window_start) >= window {
            *record = Record::new(now);
        }

        let reset_at = record.window_start.checked_add(window);
        if record.count >= self.config.limit {
            return Decision {
                allowed: false,
                remaining: 0,
                reset_at,
            };
        }

        record.count += 1;
        Decision {
            allowed: true,
            remaining: self.config.limit - record.count,
            reset_at,
        }
    }

    fn cleanup_at(&self, now: Instant) {
        let retention = self.config.window.saturating_mul(RETAINED_WINDOWS);

        self.records
            .retain(|_, record| now.saturating_duration_since(record.window_start) < retention);
    }
}

impl<K> Clone for RateLimiter<K> {
    fn clone(&self) -> Self {
        Self {
            config: self.config,
            records: Arc::clone(&self.records),
        }
    }
}

impl<K: Eq + Hash> fmt::Debug for RateLimiter<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RateLimiter")
            .field("config", &self.config)
            .field("keys", &self.records.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use uuid::Uuid;

    fn limiter<K: Eq + Hash>(limit: u32, window: Duration) -> RateLimiter<K> {
        RateLimiter::new(RateLimitConfig { limit, window })
    }

    #[test]
    fn rate_limit_allows_requests_within_limit() {
        let limiter = limiter(3, Duration::from_secs(60));
        let user = Uuid::new_v4();
        let now = Instant::now();

        for remaining in [2, 1, 0] {
            let decision = limiter.check_rate_limit_at(user, now);
            assert!(decision.allowed);
            assert_eq!(decision.remaining, remaining);
        }
    }

    #[test]
    fn rate_limit_blocks_requests_over_limit() {
        let limiter = limiter(2, Duration::from_secs(60));
        let user = Uuid::new_v4();
        let now = Instant::now();

        assert!(limiter.check_rate_limit_at(user, now).allowed);
        assert!(limiter.check_rate_limit_at(user, now).allowed);

        let decision = limiter.check_rate_limit_at(user, now);
        assert!(!decision.allowed);
        assert_eq!(decision.remaining, 0);
    }

    #[test]
    fn a_refused_request_does_not_count_against_the_window() {
        let limiter = limiter(1, Duration::from_secs(60));
        let user = Uuid::new_v4();
        let now = Instant::now();

        assert!(limiter.check_rate_limit_at(user, now).allowed);
        for _ in 0..5 {
            assert!(!limiter.check_rate_limit_at(user, now).allowed);
        }

        assert_eq!(
            limiter.records.get(&user).map(|record| record.count),
            Some(1)
        );
    }

    #[test]
    fn rate_limit_resets_after_window() {
        let window = Duration::from_secs(60);
        let limiter = limiter(2, window);
        let user = Uuid::new_v4();
        let start = Instant::now();

        assert!(limiter.check_rate_limit_at(user, start).allowed);
        assert!(limiter.check_rate_limit_at(user, start).allowed);
        assert!(
            !limiter
                .check_rate_limit_at(user, start + window - Duration::from_nanos(1))
                .allowed
        );

        let decision = limiter.check_rate_limit_at(user, start + window);
        assert!(decision.allowed);
        assert_eq!(decision.remaining, 1);
        assert_eq!(decision.reset_at, Some(start + window + window));
    }

    #[test]
    fn rate_limit_different_users_independent() {
        let limiter = limiter(1, Duration::from_secs(60));
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let now = Instant::now();

        assert!(limiter.check_rate_limit_at(first, now).allowed);
        assert!(!limiter.check_rate_limit_at(first, now).allowed);

        assert!(limiter.check_rate_limit_at(second, now).allowed);
    }

    #[test]
    fn rate_limit_cleanup_removes_old_entries() {
        let window = Duration::from_secs(60);
        let limiter = limiter(10, window);
        let start = Instant::now();

        for _ in 0..5 {
            limiter.check_rate_limit_at(Uuid::new_v4(), start);
        }
        assert_eq!(limiter.records.len(), 5);

        limiter.cleanup_at(start + window * 2 - Duration::from_nanos(1));
        assert_eq!(limiter.records.len(), 5, "a record was dropped early");

        limiter.cleanup_at(start + window * 2);
        assert_eq!(limiter.records.len(), 0);

        assert!(
            limiter
                .check_rate_limit_at(Uuid::new_v4(), start + window * 2)
                .allowed
        );
    }

    #[test]
    fn rate_limit_reset_timestamp() {
        let config = RateLimitConfig {
            limit: 1,
            window: Duration::from_secs(60),
        };
        let limiter: RateLimiter<Uuid> = RateLimiter::new(config);
        let start = Instant::now();

        let decision = limiter.check_rate_limit_at(Uuid::new_v4(), start);

        assert_eq!(decision.reset_at, Some(start + config.window));
    }

    #[test]
    fn a_window_too_long_to_represent_never_resets_rather_than_panicking() {
        let limiter = limiter(1, Duration::MAX);
        let start = Instant::now();

        let decision = limiter.check_rate_limit_at("key", start);
        assert!(decision.allowed);
        assert_eq!(decision.reset_at, None);
        assert!(
            !limiter
                .check_rate_limit_at("key", start + Duration::from_secs(86_400))
                .allowed
        );

        limiter.cleanup_at(start + Duration::from_secs(86_400));
        limiter.cleanup();
        assert_eq!(limiter.records.len(), 1);
    }

    #[test]
    fn the_public_entry_points_use_the_clock() {
        let limiter = limiter(1, Duration::MAX);

        assert!(limiter.check_rate_limit("key").allowed);
        assert!(!limiter.check_rate_limit("key").allowed);
        limiter.cleanup();
        assert_eq!(limiter.records.len(), 1);
    }

    #[test]
    fn a_limiter_counts_against_whatever_key_it_was_given() {
        let limiter = limiter(1, Duration::from_secs(60));
        let now = Instant::now();

        assert!(limiter.check_rate_limit_at("api.example", now).allowed);
        assert!(!limiter.check_rate_limit_at("api.example", now).allowed);
        assert!(limiter.check_rate_limit_at("other.example", now).allowed);
    }

    #[test]
    fn the_configuration_is_readable_back() {
        let config = RateLimitConfig::default();
        let limiter: RateLimiter<Uuid> = RateLimiter::new(config);

        assert_eq!(*limiter.config(), config);
    }

    #[test]
    fn debug_output_counts_keys_without_printing_them() {
        let limiter = limiter(10, Duration::from_secs(60));
        limiter.check_rate_limit("tenant-secret".to_string());
        limiter.check_rate_limit("api-key-0123456789".to_string());

        let debug = format!("{limiter:?}");

        assert!(!debug.contains("tenant-secret"), "{debug}");
        assert!(!debug.contains("api-key-0123456789"), "{debug}");
        assert!(debug.contains("keys: 2"), "{debug}");
        assert!(debug.contains("limit: 10"), "{debug}");
    }

    #[test]
    fn a_clone_shares_its_counts_even_for_a_key_that_cannot_be_cloned() {
        #[derive(PartialEq, Eq, Hash)]
        struct Tenant(u8);

        let limiter = limiter(1, Duration::from_secs(60));
        let clone = limiter.clone();
        let now = Instant::now();

        assert!(limiter.check_rate_limit_at(Tenant(1), now).allowed);
        assert!(!clone.check_rate_limit_at(Tenant(1), now).allowed);
    }
}
