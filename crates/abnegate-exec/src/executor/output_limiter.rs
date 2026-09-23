use super::admission::Admission;

/// Tracks output limits and truncation state for a single job, across all of
/// its streams.
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

    /// Count `incoming` bytes against the limit and say how many of them to
    /// deliver.
    pub fn admit(&mut self, incoming: usize) -> Admission {
        let accepted = incoming.min(self.max_bytes.saturating_sub(self.bytes_written));
        self.bytes_written += accepted;
        let truncated = accepted < incoming;
        let first_truncation = truncated && !self.truncation_warned;
        self.truncation_warned |= truncated;
        Admission {
            accepted,
            first_truncation,
        }
    }

    /// [`OutputLimiter::admit`] as a tuple.
    ///
    /// Returns `(can_write, bytes_to_write, should_warn)` where:
    /// - `can_write`: whether any bytes could be written before this chunk
    /// - `bytes_to_write`: how many bytes of the input to actually write
    /// - `should_warn`: whether to emit a truncation warning
    pub fn check(&mut self, incoming_bytes: usize) -> (bool, usize, bool) {
        let can_write = self.bytes_written < self.max_bytes;
        let admission = self.admit(incoming_bytes);
        (can_write, admission.accepted, admission.first_truncation)
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

    #[test]
    fn a_zero_limit_still_warns_once() {
        let mut limiter = OutputLimiter::new(0);

        assert_eq!(
            limiter.admit(5),
            Admission {
                accepted: 0,
                first_truncation: true,
            }
        );
        assert_eq!(
            limiter.admit(5),
            Admission {
                accepted: 0,
                first_truncation: false,
            }
        );
    }

    #[test]
    fn an_empty_chunk_is_never_a_truncation() {
        let mut limiter = OutputLimiter::new(0);

        assert!(!limiter.admit(0).first_truncation);
        assert!(!limiter.was_truncated());
    }
}
