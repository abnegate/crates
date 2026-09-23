use thiserror::Error;

use crate::protocol::ErrorCode;

/// Errors related to job management.
#[derive(Debug, Error)]
#[non_exhaustive]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_error_not_found() {
        let error = JobError::NotFound("job-123".to_string());
        assert_eq!(error.to_error_code(), ErrorCode::JobNotFound);
        assert!(error.to_string().contains("job-123"));
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn test_job_error_already_exists() {
        let error = JobError::AlreadyExists("job-456".to_string());
        assert_eq!(error.to_error_code(), ErrorCode::InternalError);
        assert!(error.to_string().contains("job-456"));
        assert!(error.to_string().contains("already exists"));
    }

    #[test]
    fn test_job_error_invalid_state() {
        let error = JobError::InvalidState("cannot cancel completed job".to_string());
        assert_eq!(error.to_error_code(), ErrorCode::InternalError);
        assert!(error.to_string().contains("Invalid job state"));
    }
}
