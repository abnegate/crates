use std::io;

use thiserror::Error;

use crate::executor::ConfinementError;
use crate::protocol::ErrorCode;

/// Errors that can occur during command execution.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// Failed to spawn the command process
    #[error("Failed to spawn process")]
    SpawnFailed(#[source] io::Error),

    /// Command timed out
    #[error("Command timed out after {0}ms")]
    Timeout(u64),

    /// Command was cancelled
    #[error("Command was cancelled")]
    Cancelled,

    /// Output limit exceeded
    #[error("Output limit exceeded: {written} bytes (max: {max})")]
    OutputLimitExceeded { written: usize, max: usize },

    /// Invalid workspace path
    #[error("Invalid workspace path: {0}")]
    InvalidWorkspace(String),

    /// Failed to set up process group
    #[error("Failed to set up process group: {0}")]
    ProcessGroupFailed(String),

    /// Confinement was requested but could not be established
    #[error("Confinement unavailable: {0}")]
    ConfinementUnavailable(#[from] ConfinementError),

    /// I/O error during execution
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Channel send error
    #[error("Channel closed")]
    ChannelClosed,
}

impl ExecutorError {
    /// Convert to protocol error code
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            ExecutorError::SpawnFailed(_) => ErrorCode::SpawnFailed,
            ExecutorError::Timeout(_) => ErrorCode::Timeout,
            ExecutorError::Cancelled => ErrorCode::Cancelled,
            ExecutorError::OutputLimitExceeded { .. } => ErrorCode::OutputLimitExceeded,
            ExecutorError::InvalidWorkspace(_) => ErrorCode::InvalidWorkspace,
            ExecutorError::ProcessGroupFailed(_) => ErrorCode::InternalError,
            ExecutorError::ConfinementUnavailable(_) => ErrorCode::ConfinementUnavailable,
            ExecutorError::Io(_) => ErrorCode::InternalError,
            ExecutorError::ChannelClosed => ErrorCode::InternalError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_spawn_failed() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "command not found");
        let err = ExecutorError::SpawnFailed(io_err);
        assert_eq!(err.to_error_code(), ErrorCode::SpawnFailed);
        assert!(err.to_string().contains("Failed to spawn process"));
    }

    #[test]
    fn a_spawn_failure_names_its_cause_once() {
        let error = ExecutorError::SpawnFailed(io::Error::new(
            io::ErrorKind::NotFound,
            "command not found",
        ));

        assert!(!error.to_string().contains("command not found"), "{error}");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("command not found".to_string())
        );
    }

    #[test]
    fn test_executor_error_timeout() {
        let err = ExecutorError::Timeout(5000);
        assert_eq!(err.to_error_code(), ErrorCode::Timeout);
        assert!(err.to_string().contains("5000ms"));
    }

    #[test]
    fn test_executor_error_cancelled() {
        let err = ExecutorError::Cancelled;
        assert_eq!(err.to_error_code(), ErrorCode::Cancelled);
        assert!(err.to_string().contains("cancelled"));
    }

    #[test]
    fn test_executor_error_output_limit_exceeded() {
        let err = ExecutorError::OutputLimitExceeded {
            written: 1000,
            max: 500,
        };
        assert_eq!(err.to_error_code(), ErrorCode::OutputLimitExceeded);
        assert!(err.to_string().contains("1000"));
        assert!(err.to_string().contains("500"));
    }

    #[test]
    fn test_executor_error_invalid_workspace() {
        let err = ExecutorError::InvalidWorkspace("/bad/path".to_string());
        assert_eq!(err.to_error_code(), ErrorCode::InvalidWorkspace);
        assert!(err.to_string().contains("/bad/path"));
    }

    #[test]
    fn test_executor_error_process_group_failed() {
        let err = ExecutorError::ProcessGroupFailed("setsid failed".to_string());
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("setsid failed"));
    }

    #[test]
    fn test_executor_error_io() {
        let io_err = io::Error::new(io::ErrorKind::BrokenPipe, "pipe broken");
        let err = ExecutorError::Io(io_err);
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_executor_error_channel_closed() {
        let err = ExecutorError::ChannelClosed;
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("Channel closed"));
    }

    #[test]
    fn test_executor_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let err: ExecutorError = io_err.into();
        match err {
            ExecutorError::Io(_) => {}
            _ => panic!("Expected Io variant"),
        }
    }
}
