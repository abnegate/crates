use std::time::Duration;

use crate::proxy::Proxy;

use super::environment_policy::EnvironmentPolicy;
use super::niceness::Niceness;

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

    /// The environment a command is given beneath its request's own.
    /// Defaults to the [`allowlist`](EnvironmentPolicy::allowlist) of
    /// [`DEFAULT_ENVIRONMENT`](super::DEFAULT_ENVIRONMENT).
    pub environment: EnvironmentPolicy,

    /// Where an unconfined command's HTTP clients are routed, set over its
    /// environment last so the request cannot route around it. A confined
    /// command reaches no network and never sees it. Defaults to
    /// [`Proxy::from_environment`], read when the config is built.
    pub proxy: Proxy,

    /// The nice value every command starts at, and so the whole tree it
    /// starts in turn. Nothing by default, which leaves each at the
    /// executor's own priority.
    pub niceness: Option<Niceness>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            buffer_size: DEFAULT_BUFFER_SIZE,
            grace_period: GRACE_PERIOD,
            environment: EnvironmentPolicy::default(),
            proxy: Proxy::from_environment(),
            niceness: None,
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

    /// Route every unconfined command's HTTP clients through `proxy` instead
    /// of the one the executor's environment names; [`Proxy::default`] routes
    /// nothing.
    pub fn with_proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = proxy;
        self
    }

    /// Start every command, and everything it starts, at `niceness`.
    pub fn with_niceness(mut self, niceness: Niceness) -> Self {
        self.niceness = Some(niceness);
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
        assert_eq!(config.niceness, None);
    }

    #[test]
    fn test_executor_config_builder() {
        let config = ExecutorConfig::new()
            .with_timeout(Duration::from_secs(60))
            .with_output_limit(1024)
            .with_buffer_size(512)
            .with_niceness(Niceness::LOWEST);

        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.output_limit, 1024);
        assert_eq!(config.buffer_size, 512);
        assert_eq!(config.niceness, Some(Niceness::LOWEST));
    }
}
