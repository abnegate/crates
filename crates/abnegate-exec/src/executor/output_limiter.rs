/// Tracks output limits and truncation state for a single job.
#[derive(Debug)]
pub struct OutputLimiter {
    /// Maximum bytes allowed
    max_bytes: usize,

    /// Total bytes written so far
    bytes_written: usize,

    /// Whether we've already emitted a truncation warning
    truncation_warned: bool,
}

impl OutputLimiter {
    /// Create a new output limiter
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            bytes_written: 0,
            truncation_warned: false,
        }
    }

    /// Check if more output can be written.
    ///
    /// Returns `(can_write, bytes_to_write, should_warn)` where:
    /// - `can_write`: whether any bytes can be written
    /// - `bytes_to_write`: how many bytes of the input to actually write
    /// - `should_warn`: whether to emit a truncation warning
    pub fn check(&mut self, incoming_bytes: usize) -> (bool, usize, bool) {
        if self.bytes_written >= self.max_bytes {
            return (false, 0, false);
        }

        let remaining = self.max_bytes - self.bytes_written;

        if incoming_bytes <= remaining {
            self.bytes_written += incoming_bytes;
            (true, incoming_bytes, false)
        } else {
            let should_warn = !self.truncation_warned;
            self.truncation_warned = true;
            self.bytes_written = self.max_bytes;
            (true, remaining, should_warn)
        }
    }

    /// Get total bytes written so far
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Check if output was truncated
    pub fn was_truncated(&self) -> bool {
        self.truncation_warned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_limiter_under_limit() {
        let mut limiter = OutputLimiter::new(1000);

        let (can_write, bytes, warn) = limiter.check(100);
        assert!(can_write);
        assert_eq!(bytes, 100);
        assert!(!warn);

        let (can_write, bytes, warn) = limiter.check(500);
        assert!(can_write);
        assert_eq!(bytes, 500);
        assert!(!warn);

        assert_eq!(limiter.bytes_written(), 600);
        assert!(!limiter.was_truncated());
    }

    #[test]
    fn test_output_limiter_at_limit() {
        let mut limiter = OutputLimiter::new(100);

        let (can_write, bytes, warn) = limiter.check(100);
        assert!(can_write);
        assert_eq!(bytes, 100);
        assert!(!warn);

        let (can_write, _, _) = limiter.check(10);
        assert!(!can_write);
    }

    #[test]
    fn test_output_limiter_truncation() {
        let mut limiter = OutputLimiter::new(100);

        limiter.check(50);

        let (can_write, bytes, warn) = limiter.check(100);
        assert!(can_write);
        assert_eq!(bytes, 50);
        assert!(warn);

        assert!(limiter.was_truncated());

        let (can_write, _, warn) = limiter.check(10);
        assert!(!can_write);
        assert!(!warn);
    }
}
