//! Job state machine for tracking execution lifecycle.

use std::time::Duration;
use std::time::Instant;

use crate::protocol::ErrorCode;

/// Represents the current state of a job.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum JobState {
    /// Job is being set up
    Starting { created_at: Instant },

    /// Job is actively running
    Running { pid: u32, started_at: Instant },

    /// Job completed successfully
    Completed { exit_code: i32, duration: Duration },

    /// Job terminated by signal
    Signaled { signal: i32, duration: Duration },

    /// Job failed with an error
    Failed {
        error_code: ErrorCode,
        message: String,
        duration: Duration,
    },

    /// Job was cancelled
    Cancelled { forced: bool, duration: Duration },

    /// Job timed out
    TimedOut { timeout_ms: u64, duration: Duration },
}

impl JobState {
    /// Create a new job in the Starting state
    pub fn new() -> Self {
        JobState::Starting {
            created_at: Instant::now(),
        }
    }

    /// Transition to Running state
    pub fn running(pid: u32) -> Self {
        JobState::Running {
            pid,
            started_at: Instant::now(),
        }
    }

    /// Transition to Completed state
    pub fn completed(exit_code: i32, duration: Duration) -> Self {
        JobState::Completed {
            exit_code,
            duration,
        }
    }

    /// Transition to Signaled state
    pub fn signaled(signal: i32, duration: Duration) -> Self {
        JobState::Signaled { signal, duration }
    }

    /// Transition to Failed state
    pub fn failed(error_code: ErrorCode, message: String, duration: Duration) -> Self {
        JobState::Failed {
            error_code,
            message,
            duration,
        }
    }

    /// Transition to Cancelled state
    pub fn cancelled(forced: bool, duration: Duration) -> Self {
        JobState::Cancelled { forced, duration }
    }

    /// Transition to TimedOut state
    pub fn timed_out(timeout_ms: u64, duration: Duration) -> Self {
        JobState::TimedOut {
            timeout_ms,
            duration,
        }
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Completed { .. }
                | JobState::Signaled { .. }
                | JobState::Failed { .. }
                | JobState::Cancelled { .. }
                | JobState::TimedOut { .. }
        )
    }

    /// Check if the job is still running
    pub fn is_running(&self) -> bool {
        matches!(self, JobState::Running { .. })
    }

    /// Get the process ID if running
    pub fn pid(&self) -> Option<u32> {
        match self {
            JobState::Running { pid, .. } => Some(*pid),
            _ => None,
        }
    }

    /// Get the duration if in a terminal state
    pub fn duration(&self) -> Option<Duration> {
        match self {
            JobState::Completed { duration, .. } => Some(*duration),
            JobState::Signaled { duration, .. } => Some(*duration),
            JobState::Failed { duration, .. } => Some(*duration),
            JobState::Cancelled { duration, .. } => Some(*duration),
            JobState::TimedOut { duration, .. } => Some(*duration),
            _ => None,
        }
    }

    /// Get the error code if failed
    pub fn error_code(&self) -> Option<ErrorCode> {
        match self {
            JobState::Failed { error_code, .. } => Some(*error_code),
            JobState::Cancelled { .. } => Some(ErrorCode::Cancelled),
            JobState::TimedOut { .. } => Some(ErrorCode::Timeout),
            _ => None,
        }
    }
}

impl Default for JobState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_state_new() {
        let state = JobState::new();
        assert!(!state.is_terminal());
        assert!(!state.is_running());
        assert!(state.pid().is_none());
        assert!(state.duration().is_none());
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_job_state_default() {
        let state: JobState = Default::default();
        assert!(!state.is_terminal());
        assert!(!state.is_running());
    }

    #[test]
    fn test_job_state_running() {
        let state = JobState::running(12345);
        assert!(!state.is_terminal());
        assert!(state.is_running());
        assert_eq!(state.pid(), Some(12345));
        assert!(state.duration().is_none());
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_job_state_completed() {
        let state = JobState::completed(0, Duration::from_secs(5));
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert!(state.pid().is_none());
        assert_eq!(state.duration(), Some(Duration::from_secs(5)));
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_job_state_completed_non_zero() {
        let state = JobState::completed(1, Duration::from_millis(500));
        assert!(state.is_terminal());
        assert_eq!(state.duration(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_job_state_signaled() {
        let state = JobState::signaled(9, Duration::from_secs(2));
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert!(state.pid().is_none());
        assert_eq!(state.duration(), Some(Duration::from_secs(2)));
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_job_state_failed() {
        let state = JobState::failed(
            ErrorCode::SpawnFailed,
            "command not found".to_string(),
            Duration::from_millis(10),
        );
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert_eq!(state.duration(), Some(Duration::from_millis(10)));
        assert_eq!(state.error_code(), Some(ErrorCode::SpawnFailed));
    }

    #[test]
    fn test_job_state_cancelled() {
        let state = JobState::cancelled(false, Duration::from_secs(3));
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert_eq!(state.duration(), Some(Duration::from_secs(3)));
        assert_eq!(state.error_code(), Some(ErrorCode::Cancelled));
    }

    #[test]
    fn test_job_state_cancelled_forced() {
        let state = JobState::cancelled(true, Duration::from_secs(1));
        assert!(state.is_terminal());
        assert_eq!(state.error_code(), Some(ErrorCode::Cancelled));
    }

    #[test]
    fn test_job_state_timed_out() {
        let state = JobState::timed_out(30000, Duration::from_secs(30));
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert_eq!(state.duration(), Some(Duration::from_secs(30)));
        assert_eq!(state.error_code(), Some(ErrorCode::Timeout));
    }

    #[test]
    fn test_state_transitions() {
        let state = JobState::new();
        assert!(!state.is_terminal());
        assert!(!state.is_running());

        let state = JobState::running(12345);
        assert!(!state.is_terminal());
        assert!(state.is_running());
        assert_eq!(state.pid(), Some(12345));

        let state = JobState::completed(0, Duration::from_secs(1));
        assert!(state.is_terminal());
        assert!(!state.is_running());
        assert_eq!(state.duration(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn test_is_terminal_starting() {
        let state = JobState::new();
        assert!(!state.is_terminal());
    }

    #[test]
    fn test_is_terminal_running() {
        let state = JobState::running(1);
        assert!(!state.is_terminal());
    }

    #[test]
    fn test_is_terminal_completed() {
        let state = JobState::completed(0, Duration::from_secs(1));
        assert!(state.is_terminal());
    }

    #[test]
    fn test_is_terminal_signaled() {
        let state = JobState::signaled(15, Duration::from_secs(1));
        assert!(state.is_terminal());
    }

    #[test]
    fn test_is_terminal_failed() {
        let state = JobState::failed(
            ErrorCode::InternalError,
            "err".to_string(),
            Duration::from_secs(1),
        );
        assert!(state.is_terminal());
    }

    #[test]
    fn test_is_terminal_cancelled() {
        let state = JobState::cancelled(false, Duration::from_secs(1));
        assert!(state.is_terminal());
    }

    #[test]
    fn test_is_terminal_timed_out() {
        let state = JobState::timed_out(1000, Duration::from_secs(1));
        assert!(state.is_terminal());
    }

    #[test]
    fn test_is_running_starting() {
        let state = JobState::new();
        assert!(!state.is_running());
    }

    #[test]
    fn test_is_running_running() {
        let state = JobState::running(123);
        assert!(state.is_running());
    }

    #[test]
    fn test_is_running_completed() {
        let state = JobState::completed(0, Duration::from_secs(1));
        assert!(!state.is_running());
    }

    #[test]
    fn test_pid_starting() {
        let state = JobState::new();
        assert!(state.pid().is_none());
    }

    #[test]
    fn test_pid_running() {
        let state = JobState::running(42);
        assert_eq!(state.pid(), Some(42));
    }

    #[test]
    fn test_pid_completed() {
        let state = JobState::completed(0, Duration::from_secs(1));
        assert!(state.pid().is_none());
    }

    #[test]
    fn test_pid_various_values() {
        assert_eq!(JobState::running(1).pid(), Some(1));
        assert_eq!(JobState::running(65535).pid(), Some(65535));
        assert_eq!(JobState::running(u32::MAX).pid(), Some(u32::MAX));
    }

    #[test]
    fn test_duration_starting() {
        let state = JobState::new();
        assert!(state.duration().is_none());
    }

    #[test]
    fn test_duration_running() {
        let state = JobState::running(1);
        assert!(state.duration().is_none());
    }

    #[test]
    fn test_duration_completed() {
        let dur = Duration::from_millis(12345);
        let state = JobState::completed(0, dur);
        assert_eq!(state.duration(), Some(dur));
    }

    #[test]
    fn test_duration_signaled() {
        let dur = Duration::from_secs(100);
        let state = JobState::signaled(9, dur);
        assert_eq!(state.duration(), Some(dur));
    }

    #[test]
    fn test_duration_failed() {
        let dur = Duration::from_millis(50);
        let state = JobState::failed(ErrorCode::SpawnFailed, "err".to_string(), dur);
        assert_eq!(state.duration(), Some(dur));
    }

    #[test]
    fn test_duration_cancelled() {
        let dur = Duration::from_secs(5);
        let state = JobState::cancelled(true, dur);
        assert_eq!(state.duration(), Some(dur));
    }

    #[test]
    fn test_duration_timed_out() {
        let dur = Duration::from_secs(30);
        let state = JobState::timed_out(30000, dur);
        assert_eq!(state.duration(), Some(dur));
    }

    #[test]
    fn test_error_code_starting() {
        let state = JobState::new();
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_error_code_running() {
        let state = JobState::running(1);
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_error_code_completed() {
        let state = JobState::completed(0, Duration::from_secs(1));
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_error_code_signaled() {
        let state = JobState::signaled(9, Duration::from_secs(1));
        assert!(state.error_code().is_none());
    }

    #[test]
    fn test_error_code_failed_various() {
        let codes = [
            ErrorCode::SpawnFailed,
            ErrorCode::Timeout,
            ErrorCode::Cancelled,
            ErrorCode::OutputLimitExceeded,
            ErrorCode::InvalidWorkspace,
            ErrorCode::InternalError,
            ErrorCode::JobNotFound,
            ErrorCode::InvalidMessage,
        ];

        for code in codes {
            let state = JobState::failed(code, "test".to_string(), Duration::from_secs(1));
            assert_eq!(state.error_code(), Some(code));
        }
    }

    #[test]
    fn test_error_code_cancelled() {
        let state = JobState::cancelled(false, Duration::from_secs(1));
        assert_eq!(state.error_code(), Some(ErrorCode::Cancelled));
    }

    #[test]
    fn test_error_code_timed_out() {
        let state = JobState::timed_out(1000, Duration::from_secs(1));
        assert_eq!(state.error_code(), Some(ErrorCode::Timeout));
    }

    #[test]
    fn test_clone_starting() {
        let state = JobState::new();
        let cloned = state.clone();
        assert!(!cloned.is_terminal());
    }

    #[test]
    fn test_clone_running() {
        let state = JobState::running(12345);
        let cloned = state.clone();
        assert_eq!(cloned.pid(), Some(12345));
    }

    #[test]
    fn test_clone_completed() {
        let state = JobState::completed(42, Duration::from_secs(10));
        let cloned = state.clone();
        assert!(cloned.is_terminal());
        assert_eq!(cloned.duration(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn test_debug_format() {
        let state = JobState::running(123);
        let debug = format!("{:?}", state);
        assert!(debug.contains("Running"));
        assert!(debug.contains("123"));
    }

    #[test]
    fn test_error_states() {
        let state = JobState::cancelled(false, Duration::from_secs(2));
        assert!(state.is_terminal());
        assert_eq!(state.error_code(), Some(ErrorCode::Cancelled));

        let state = JobState::timed_out(5000, Duration::from_secs(5));
        assert!(state.is_terminal());
        assert_eq!(state.error_code(), Some(ErrorCode::Timeout));

        let state = JobState::failed(
            ErrorCode::SpawnFailed,
            "test".to_string(),
            Duration::from_millis(100),
        );
        assert!(state.is_terminal());
        assert_eq!(state.error_code(), Some(ErrorCode::SpawnFailed));
    }
}
