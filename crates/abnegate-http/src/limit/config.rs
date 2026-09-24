use std::time::Duration;

/// How many requests a key may make, and over what window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// How many requests a key may make in one window.
    pub limit: u32,
    /// The length of a window.
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            limit: 10,
            window: Duration::from_secs(60),
        }
    }
}
