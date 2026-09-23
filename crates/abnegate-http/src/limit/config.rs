use std::time::Duration;

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
