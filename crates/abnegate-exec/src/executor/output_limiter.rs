use super::admission::Admission;

/// Tracks output limits and truncation state for a single job, across all of
/// its streams.
#[derive(Debug)]
pub struct OutputLimiter {
    /// Most bytes delivered
    limit: usize,

    /// Total bytes written so far
    bytes_written: usize,

    /// Whether we've already emitted a truncation warning
    truncation_warned: bool,
}

impl OutputLimiter {
    /// Deliver at most `limit` bytes
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes_written: 0,
            truncation_warned: false,
        }
    }

    /// Count `incoming` bytes against the limit and say how many of them to
    /// deliver.
    pub fn admit(&mut self, incoming: usize) -> Admission {
        let accepted = incoming.min(self.limit.saturating_sub(self.bytes_written));
        self.bytes_written += accepted;
        let truncated = accepted < incoming;
        let first_truncation = truncated && !self.truncation_warned;
        self.truncation_warned |= truncated;
        Admission {
            accepted,
            first_truncation,
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

    fn admitted(accepted: usize, first_truncation: bool) -> Admission {
        Admission {
            accepted,
            first_truncation,
        }
    }

    #[test]
    fn test_output_limiter_under_limit() {
        let mut limiter = OutputLimiter::new(1000);

        assert_eq!(limiter.admit(100), admitted(100, false));
        assert_eq!(limiter.admit(500), admitted(500, false));
        assert_eq!(limiter.bytes_written(), 600);
        assert!(!limiter.was_truncated());
    }

    #[test]
    fn test_output_limiter_at_limit() {
        let mut limiter = OutputLimiter::new(100);

        assert_eq!(limiter.admit(100), admitted(100, false));
        assert_eq!(limiter.admit(10), admitted(0, true));
    }

    #[test]
    fn test_output_limiter_truncation() {
        let mut limiter = OutputLimiter::new(100);
        limiter.admit(50);

        assert_eq!(limiter.admit(100), admitted(50, true));
        assert!(limiter.was_truncated());
        assert_eq!(limiter.admit(10), admitted(0, false));
    }

    #[test]
    fn a_zero_limit_still_warns_once() {
        let mut limiter = OutputLimiter::new(0);

        assert_eq!(limiter.admit(5), admitted(0, true));
        assert_eq!(limiter.admit(5), admitted(0, false));
    }

    #[test]
    fn an_empty_chunk_is_never_a_truncation() {
        let mut limiter = OutputLimiter::new(0);

        assert!(!limiter.admit(0).first_truncation);
        assert!(!limiter.was_truncated());
    }
}
