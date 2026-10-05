use std::io;

use thiserror::Error;

/// Errors from sweeping the processes that carry a [`Marker`](crate::Marker).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MarkerError {
    /// A marker's variable name was empty or held `=` or a NUL byte, so no
    /// environment entry could carry it
    #[error("Not an environment variable name: {0:?}")]
    InvalidVariable(String),

    /// The host's processes could not be listed, so none could be checked
    /// for the marker
    #[error("Failed to list processes")]
    ListFailed(#[source] io::Error),

    /// A process carrying the marker could not be signalled
    #[error("Failed to signal process {pid}")]
    #[non_exhaustive]
    SignalFailed {
        /// The process that carried the marker
        pid: u32,
        /// Why the signal was refused
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn an_invalid_variable_names_itself() {
        let error = MarkerError::InvalidVariable("RUN=".to_string());

        assert_eq!(
            error.to_string(),
            r#"Not an environment variable name: "RUN=""#
        );
    }

    #[test]
    fn a_listing_failure_keeps_its_cause() {
        let error = MarkerError::ListFailed(io::Error::from(io::ErrorKind::PermissionDenied));

        assert_eq!(error.to_string(), "Failed to list processes");
        assert!(error.source().is_some());
    }

    #[test]
    fn a_signal_failure_names_the_process() {
        let error = MarkerError::SignalFailed {
            pid: 42,
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };

        assert_eq!(error.to_string(), "Failed to signal process 42");
        assert!(error.source().is_some());
    }
}
