use std::time::Duration;

use super::environment_policy::EnvironmentPolicy;

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

    /// Maximum bytes of stdout and stderr together to deliver. Output past it
    /// is still read, so the command runs to completion, and then dropped.
    pub max_output_bytes: usize,

    /// Largest chunk read from a pipe at once, and so the largest payload of
    /// a single `RunStdout` or `RunStderr`
    pub buffer_size: usize,

    /// Grace period before SIGKILL after SIGTERM
    pub grace_period: Duration,

    /// Which of the executor's own environment variables a command sees.
    /// Defaults to [`EnvironmentPolicy::Allowlist`] of
    /// [`DEFAULT_ENVIRONMENT_ALLOWLIST`](super::DEFAULT_ENVIRONMENT_ALLOWLIST).
    pub environment: EnvironmentPolicy,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            buffer_size: DEFAULT_BUFFER_SIZE,
            grace_period: GRACE_PERIOD,
            environment: EnvironmentPolicy::default(),
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

    /// Set which of the executor's own environment variables a command sees
    pub fn with_environment(mut self, environment: EnvironmentPolicy) -> Self {
        self.environment = environment;
        self
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
}
