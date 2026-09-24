use std::time::Duration;

use super::environment_policy::EnvironmentPolicy;

/// How long a command may run when its `RunStart` sets no timeout: five
/// minutes
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How many bytes of stdout and stderr together a command may deliver when
/// its `RunStart` sets no limit: 10 MiB
pub const DEFAULT_OUTPUT_LIMIT: usize = 10 * 1024 * 1024;

/// Default buffer size for reading output (8 KB)
pub const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;

/// Grace period for process termination before SIGKILL
pub const GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Configuration for the command executor.
///
/// Start from [`ExecutorConfig::new`] or `Default` and adjust it with the
/// `with_*` methods.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExecutorConfig {
    /// How long a command may run when its `RunStart` sets no timeout
    pub timeout: Duration,

    /// Most bytes of stdout and stderr together to deliver when a `RunStart`
    /// sets no limit. Output past it is still read, so the command runs to
    /// completion, and then dropped.
    pub output_limit: usize,

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
            timeout: DEFAULT_TIMEOUT,
            output_limit: DEFAULT_OUTPUT_LIMIT,
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

    /// Set how long a command may run when its `RunStart` sets no timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set how many bytes of output a command may deliver when its
    /// `RunStart` sets no limit
    pub fn with_output_limit(mut self, output_limit: usize) -> Self {
        self.output_limit = output_limit;
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
        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert_eq!(config.output_limit, DEFAULT_OUTPUT_LIMIT);
        assert_eq!(config.buffer_size, DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = ExecutorConfig::new()
            .with_timeout(Duration::from_secs(60))
            .with_output_limit(1024)
            .with_buffer_size(512);

        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.output_limit, 1024);
        assert_eq!(config.buffer_size, 512);
    }
}
