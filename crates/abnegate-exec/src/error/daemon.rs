use std::io;

use thiserror::Error;

use crate::protocol::ErrorCode;

use super::executor::ExecutorError;
use super::job::JobError;
use super::protocol::ProtocolError;

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
