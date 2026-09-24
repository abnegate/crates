//! A run that left nothing to judge.

use std::error::Error;
use std::fmt;

use abnegate_llm::ProviderError;

use crate::log::ExecutionLogFiles;

/// Why [`CliProvider::execute`](crate::CliProvider::execute) produced no
/// [`Execution`](crate::Execution), with where the run's logs are, so a run
/// that timed out or broke its stream can still be looked into.
///
/// It reads as the [`ProviderError`] inside it, and converts into one.
#[derive(Debug)]
#[non_exhaustive]
pub struct ExecutionError {
    pub error: Box<ProviderError>,
    /// The run's logs, when it kept any and got far enough to open them.
    pub log: Option<ExecutionLogFiles>,
}

impl ExecutionError {
    pub fn new(error: ProviderError, log: Option<ExecutionLogFiles>) -> Self {
        Self {
            error: Box::new(error),
            log,
        }
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

impl Error for ExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.error.source()
    }
}

impl From<ExecutionError> for ProviderError {
    fn from(error: ExecutionError) -> Self {
        *error.error
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use abnegate_llm::ProviderError;

    use super::ExecutionError;
    use crate::log::ExecutionLogFiles;

    #[test]
    fn it_reads_as_the_error_inside_it_and_keeps_the_logs() {
        let log = ExecutionLogFiles::new(
            "/tmp/run.stdout.log",
            "/tmp/run.stderr.log",
            "/tmp/run.events.jsonl",
        );
        let error = ExecutionError::new(
            ProviderError::timeout("claude", Duration::from_secs(5)),
            Some(log.clone()),
        );

        assert_eq!(
            error.to_string(),
            "claude: agent command timed out after 5s"
        );
        assert_eq!(error.log.as_ref(), Some(&log));
        assert!(matches!(
            ProviderError::from(error),
            ProviderError::Timeout { timeout, .. } if timeout == Duration::from_secs(5)
        ));
    }
}
