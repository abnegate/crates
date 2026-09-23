//! Resource limits and configuration for command execution.

use std::time::Duration;

/// Default timeout for command execution (5 minutes)
pub const DEFAULT_TIMEOUT_MS: u64 = 5 * 60 * 1000;

/// Default maximum output size (10 MB)
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Default buffer size for reading output (8 KB)
pub const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;

/// Grace period for process termination before SIGKILL
pub const GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Configuration for the command executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Default timeout for commands that don't specify one
    pub default_timeout: Duration,

    /// Maximum bytes of output to capture before truncating
    pub max_output_bytes: usize,

    /// Buffer size for reading stdout/stderr
    pub buffer_size: usize,

    /// Grace period before SIGKILL after SIGTERM
    pub grace_period: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            buffer_size: DEFAULT_BUFFER_SIZE,
            grace_period: GRACE_PERIOD,
        }
    }
}

impl ExecutorConfig {
    /// Create a new executor config with custom settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set the maximum output size
    pub fn with_max_output(mut self, max_bytes: usize) -> Self {
        self.max_output_bytes = max_bytes;
        self
    }

    /// Set the buffer size
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Set the grace period
    pub fn with_grace_period(mut self, period: Duration) -> Self {
        self.grace_period = period;
        self
    }
}

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
    fn test_executor_config_defaults() {
        let config = ExecutorConfig::default();
        assert_eq!(
            config.default_timeout,
            Duration::from_millis(DEFAULT_TIMEOUT_MS)
        );
        assert_eq!(config.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
        assert_eq!(config.buffer_size, DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = ExecutorConfig::new()
            .with_timeout(Duration::from_secs(60))
            .with_max_output(1024)
            .with_buffer_size(512);

        assert_eq!(config.default_timeout, Duration::from_secs(60));
        assert_eq!(config.max_output_bytes, 1024);
        assert_eq!(config.buffer_size, 512);
    }

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
