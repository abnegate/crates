use thiserror::Error;
use tokio::task::JoinError;

use super::ExecutorError;
use super::MarkerError;

/// Errors from stopping the runs a [`GroupRegistry`](crate::GroupRegistry)
/// holds, reported once every group has been signalled and every marker
/// swept.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GroupRegistryError {
    /// A registered process group refused a signal
    #[error("Failed to signal a registered process group")]
    Signal(#[source] ExecutorError),

    /// The sweep of a tracked marker failed
    #[error("Failed to sweep a tracked marker")]
    Sweep(#[source] MarkerError),

    /// The runtime cancelled the sweep of the tracked markers before it ran,
    /// as one does while it shuts down; the markers stay tracked for a later
    /// [`kill_all`](crate::GroupRegistry::kill_all)
    #[error("The sweep of the tracked markers was cancelled")]
    Cancelled(#[source] JoinError),
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::*;

    #[test]
    fn a_signal_failure_keeps_its_cause() {
        let error = GroupRegistryError::Signal(ExecutorError::ProcessGroupFailed(
            "Failed to send SIGKILL to process group 42: EPERM".to_string(),
        ));

        assert_eq!(
            error.to_string(),
            "Failed to signal a registered process group"
        );
        assert!(error.source().is_some());
    }

    #[test]
    fn a_sweep_failure_keeps_its_cause() {
        let error = GroupRegistryError::Sweep(MarkerError::ListFailed(io::Error::from(
            io::ErrorKind::PermissionDenied,
        )));

        assert_eq!(error.to_string(), "Failed to sweep a tracked marker");
        assert!(error.source().is_some());
    }
}
