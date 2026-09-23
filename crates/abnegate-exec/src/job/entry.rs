use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::executor::ProcessGroup;

use super::state::JobState;

/// Entry for a tracked job
pub struct JobEntry {
    /// Current state of the job
    pub state: JobState,

    /// Cancelled when the job is; pass it to
    /// [`CommandExecutor::spawn_with_cancellation`](crate::executor::CommandExecutor::spawn_with_cancellation)
    pub cancellation: CancellationToken,

    /// Process group for signaling (if running)
    pub process_group: Option<ProcessGroup>,

    /// Channel to send stdin data
    pub stdin: Option<mpsc::Sender<Vec<u8>>>,

    /// When the job was registered
    pub created_at: Instant,

    /// Whether the job was cancelled with SIGKILL rather than SIGTERM
    pub forced: bool,
}

impl JobEntry {
    /// Create a new job entry
    pub fn new() -> Self {
        Self {
            state: JobState::new(),
            cancellation: CancellationToken::new(),
            process_group: None,
            stdin: None,
            created_at: Instant::now(),
            forced: false,
        }
    }

    /// Get a clone of the cancellation token
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Move to `state`, forgetting the process group and stdin once the job
    /// has finished.
    pub(super) fn transition(&mut self, state: JobState) {
        if state.is_terminal() {
            self.process_group = None;
            self.stdin = None;
        }
        self.state = state;
    }
}

impl Default for JobEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_entry_new() {
        let entry = JobEntry::new();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancellation.is_cancelled());
        assert!(entry.process_group.is_none());
        assert!(entry.stdin.is_none());
    }

    #[test]
    fn test_job_entry_default() {
        let entry: JobEntry = Default::default();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancellation.is_cancelled());
    }

    #[test]
    fn test_job_entry_cancel_token() {
        let entry = JobEntry::new();
        let first = entry.cancel_token();
        let second = entry.cancel_token();

        first.cancel();
        assert!(second.is_cancelled());
    }
}
