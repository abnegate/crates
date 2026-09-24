use std::io;

use thiserror::Error;

use crate::protocol::ErrorCode;

use super::executor::ExecutorError;
use super::job::JobError;
use super::protocol::ProtocolError;

/// Any error a runner meets while serving one protocol message, whether from
/// the protocol, the executor, the job registry, the pipe, or a `RunStdin`
/// payload that is not base64. [`DaemonError::to_error_code`] maps each onto
/// the code a `RunError` reports.
#[derive(Debug, Error)]
#[non_exhaustive]
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
            DaemonError::Executor(error) => error.to_error_code(),
            DaemonError::Job(error) => error.to_error_code(),
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
        let io_error = io::Error::other("test");
        let protocol_error = ProtocolError::Io(io_error);
        let error = DaemonError::Protocol(protocol_error);
        assert_eq!(error.to_error_code(), ErrorCode::InvalidMessage);
        assert!(error.to_string().contains("Protocol error"));
    }

    #[test]
    fn test_daemon_error_executor() {
        let executor_error = ExecutorError::Timeout(std::time::Duration::from_secs(1));
        let error = DaemonError::Executor(executor_error);
        assert_eq!(error.to_error_code(), ErrorCode::Timeout);
        assert!(error.to_string().contains("Executor error"));
    }

    #[test]
    fn test_daemon_error_job() {
        let job_error = JobError::NotFound("test-job".to_string());
        let error = DaemonError::Job(job_error);
        assert_eq!(error.to_error_code(), ErrorCode::JobNotFound);
        assert!(error.to_string().contains("Job error"));
    }

    #[test]
    fn test_daemon_error_io() {
        let io_error = io::Error::other("some io error");
        let error = DaemonError::Io(io_error);
        assert_eq!(error.to_error_code(), ErrorCode::InternalError);
        assert!(error.to_string().contains("I/O error"));
    }

    #[test]
    fn test_daemon_error_base64() {
        let decode_error = base64::DecodeError::InvalidLength(3);
        let error = DaemonError::Base64(decode_error);
        assert_eq!(error.to_error_code(), ErrorCode::InvalidMessage);
        assert!(error.to_string().contains("Base64"));
    }

    #[test]
    fn test_daemon_error_from_protocol() {
        let io_error = io::Error::other("test");
        let protocol_error = ProtocolError::Io(io_error);
        let error: DaemonError = protocol_error.into();
        match error {
            DaemonError::Protocol(_) => {}
            _ => panic!("Expected Protocol variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_executor() {
        let executor_error = ExecutorError::Cancelled;
        let error: DaemonError = executor_error.into();
        match error {
            DaemonError::Executor(_) => {}
            _ => panic!("Expected Executor variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_job() {
        let job_error = JobError::AlreadyExists("test".to_string());
        let error: DaemonError = job_error.into();
        match error {
            DaemonError::Job(_) => {}
            _ => panic!("Expected Job variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_io() {
        let io_error = io::Error::other("test");
        let error: DaemonError = io_error.into();
        match error {
            DaemonError::Io(_) => {}
            _ => panic!("Expected Io variant"),
        }
    }

    #[test]
    fn test_daemon_error_from_base64() {
        let decode_error = base64::DecodeError::InvalidByte(0, b'!');
        let error: DaemonError = decode_error.into();
        match error {
            DaemonError::Base64(_) => {}
            _ => panic!("Expected Base64 variant"),
        }
    }
}
