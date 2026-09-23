use std::time::Instant;

/// What a [`RateLimiter`](crate::RateLimiter) decided about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// How many more requests the key may make before the window resets.
    pub remaining: u32,
    /// When the window resets, or `None` when the window is too long for an
    /// [`Instant`] to represent its end, so it never does.
    pub reset_at: Option<Instant>,
}
