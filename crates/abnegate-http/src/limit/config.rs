use std::time::Duration;

/// How many requests a key may make, and over what window.
///
/// A field may be added in a minor release, so outside this crate a
/// configuration is built with [`RateLimitConfig::new`] or taken from
/// [`RateLimitConfig::default`], never written out as a literal:
///
/// ```compile_fail,E0639
/// use abnegate_http::RateLimitConfig;
/// use std::time::Duration;
///
/// let _ = RateLimitConfig {
///     limit: 5,
///     window: Duration::from_secs(1),
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RateLimitConfig {
    /// How many requests a key may make in one window.
    pub limit: u32,
    /// The length of a window.
    pub window: Duration,
}

impl RateLimitConfig {
    /// Allow each key `limit` requests in every `window`.
    pub const fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window }
    }
}

impl Default for RateLimitConfig {
    /// Ten requests a minute.
    fn default() -> Self {
        Self::new(10, Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configuration_holds_the_limit_and_window_it_was_built_with() {
        let config = RateLimitConfig::new(3, Duration::from_millis(1_500));

        assert_eq!(config.limit, 3);
        assert_eq!(config.window, Duration::from_millis(1_500));
    }

    #[test]
    fn the_default_allows_ten_requests_a_minute() {
        assert_eq!(
            RateLimitConfig::default(),
            RateLimitConfig::new(10, Duration::from_secs(60))
        );
    }
}
