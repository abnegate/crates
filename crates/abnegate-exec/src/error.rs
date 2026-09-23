//! Error types for confined and unconfined command execution.

use crate::executor::ConfinementError;
use crate::protocol::ErrorCode;
use std::io;
use thiserror::Error;

/// Errors that can occur during protocol communication.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Failed to parse JSON message
    #[error("Failed to parse JSON: {source} (line: {line})")]
    JsonParse {
        source: serde_json::Error,
        line: String,
    },

    /// Failed to serialize JSON message
    #[error("Failed to serialize JSON: {0}")]
    JsonSerialize(#[source] serde_json::Error),

    /// Line exceeds maximum allowed length
    #[error("Line too long: {length} bytes (max: {max})")]
    LineTooLong { length: usize, max: usize },

    /// I/O error during communication
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Errors that can occur during command execution.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// Failed to spawn the command process
    #[error("Failed to spawn process: {0}")]
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

/// Errors related to job management.
#[derive(Debug, Error)]
pub enum JobError {
    /// Job not found
    #[error("Job not found: {0}")]
    NotFound(String),

    /// Job already exists
    #[error("Job already exists: {0}")]
    AlreadyExists(String),

    /// Job is in invalid state for operation
    #[error("Invalid job state for operation: {0}")]
    InvalidState(String),
}

impl JobError {
    /// Convert to protocol error code
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            JobError::NotFound(_) => ErrorCode::JobNotFound,
            JobError::AlreadyExists(_) => ErrorCode::InternalError,
            JobError::InvalidState(_) => ErrorCode::InternalError,
        }
    }
}

/// Top-level error type for the daemon.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// Protocol error
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Executor error
    #[error("Executor error: {0}")]
    Executor(#[from] ExecutorError),

    /// Job error
    #[error("Job error: {0}")]
    Job(#[from] JobError),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Base64 decode error
    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
}

impl DaemonError {
    /// Convert to protocol error code
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            DaemonError::Protocol(_) => ErrorCode::InvalidMessage,
            DaemonError::Executor(e) => e.to_error_code(),
            DaemonError::Job(e) => e.to_error_code(),
            DaemonError::Io(_) => ErrorCode::InternalError,
            DaemonError::Base64(_) => ErrorCode::InvalidMessage,
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

    #[test]
    fn test_job_error_not_found() {
        let err = JobError::NotFound("job-123".to_string());
        assert_eq!(err.to_error_code(), ErrorCode::JobNotFound);
        assert!(err.to_string().contains("job-123"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_job_error_already_exists() {
        let err = JobError::AlreadyExists("job-456".to_string());
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("job-456"));
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_job_error_invalid_state() {
        let err = JobError::InvalidState("cannot cancel completed job".to_string());
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("Invalid job state"));
    }

    #[test]
    fn test_protocol_error_json_parse() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = ProtocolError::JsonParse {
            source: json_err,
            line: "invalid".to_string(),
        };
        assert!(err.to_string().contains("Failed to parse JSON"));
        assert!(err.to_string().contains("invalid"));
    }

    #[test]
    fn test_protocol_error_json_serialize() {
        struct BadSerializer;
        impl serde::Serialize for BadSerializer {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("intentional serialization error"))
            }
        }

        let json_err = serde_json::to_string(&BadSerializer).unwrap_err();
        let err = ProtocolError::JsonSerialize(json_err);
        assert!(err.to_string().contains("Failed to serialize JSON"));
    }

    #[test]
    fn test_protocol_error_line_too_long() {
        let err = ProtocolError::LineTooLong {
            length: 2000,
            max: 1000,
        };
        assert!(err.to_string().contains("2000"));
        assert!(err.to_string().contains("1000"));
    }

    #[test]
    fn test_protocol_error_io() {
        let io_err = io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF");
        let err = ProtocolError::Io(io_err);
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_protocol_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
        let err: ProtocolError = io_err.into();
        match err {
            ProtocolError::Io(_) => {}
            _ => panic!("Expected Io variant"),
        }
    }

    #[test]
    fn test_daemon_error_protocol() {
        let io_err = io::Error::other("test");
        let protocol_err = ProtocolError::Io(io_err);
        let err = DaemonError::Protocol(protocol_err);
        assert_eq!(err.to_error_code(), ErrorCode::InvalidMessage);
        assert!(err.to_string().contains("Protocol error"));
    }

    #[test]
    fn test_daemon_error_executor() {
        let executor_err = ExecutorError::Timeout(1000);
        let err = DaemonError::Executor(executor_err);
        assert_eq!(err.to_error_code(), ErrorCode::Timeout);
        assert!(err.to_string().contains("Executor error"));
    }

    #[test]
    fn test_daemon_error_job() {
        let job_err = JobError::NotFound("test-job".to_string());
        let err = DaemonError::Job(job_err);
        assert_eq!(err.to_error_code(), ErrorCode::JobNotFound);
        assert!(err.to_string().contains("Job error"));
    }

    #[test]
    fn test_daemon_error_io() {
        let io_err = io::Error::other("some io error");
        let err = DaemonError::Io(io_err);
        assert_eq!(err.to_error_code(), ErrorCode::InternalError);
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_daemon_error_base64() {
        let b64_err = base64::DecodeError::InvalidLength(3);
        let err = DaemonError::Base64(b64_err);
        assert_eq!(err.to_error_code(), ErrorCode::InvalidMessage);
        assert!(err.to_string().contains("Base64"));
    }

    #[test]
    fn test_daemon_error_from_protocol() {
        let io_err = io::Error::other("test");
        let protocol_err = ProtocolError::Io(io_err);
        let err: DaemonError = protocol_err.into();
        match err {
            DaemonError::Protocol(_) => {}
            _ => panic!("Expected Protocol variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_executor() {
        let executor_err = ExecutorError::Cancelled;
        let err: DaemonError = executor_err.into();
        match err {
            DaemonError::Executor(_) => {}
            _ => panic!("Expected Executor variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_job() {
        let job_err = JobError::AlreadyExists("test".to_string());
        let err: DaemonError = job_err.into();
        match err {
            DaemonError::Job(_) => {}
            _ => panic!("Expected Job variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_io() {
        let io_err = io::Error::other("test");
        let err: DaemonError = io_err.into();
        match err {
            DaemonError::Io(_) => {}
            _ => panic!("Expected Io variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_base64() {
        let b64_err = base64::DecodeError::InvalidByte(0, b'!');
        let err: DaemonError = b64_err.into();
        match err {
            DaemonError::Base64(_) => {}
            _ => panic!("Expected Base64 variant"),
        }
    }
}
