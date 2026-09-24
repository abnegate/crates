use std::time::Instant;

/// What a [`RateLimiter`](crate::RateLimiter) decided about one request.
///
/// A field may be added in a minor release, so a pattern outside this crate
/// names the fields it reads and ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_http::{Decision, RateLimitConfig, RateLimiter};
///
/// let limiter = RateLimiter::new(RateLimitConfig::default());
/// let Decision {
///     allowed,
///     remaining,
///     reset_at,
/// } = limiter.check_rate_limit("tenant");
/// # let _ = (allowed, remaining, reset_at);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Decision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// How many more requests the key may make before the window resets.
    pub remaining: u32,
    /// When the window resets, or `None` when the window is too long for an
    /// [`Instant`] to represent its end, so it never does.
    pub reset_at: Option<Instant>,
}
