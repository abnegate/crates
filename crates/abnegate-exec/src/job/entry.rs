use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::executor::ProcessGroup;

use super::state::JobState;

/// Entry for a tracked job
pub struct JobEntry {
    /// Current state of the job
    pub state: JobState,

    /// Cancellation flag
    pub cancelled: Arc<AtomicBool>,

    /// Process group for signaling (if running)
    pub process_group: Option<ProcessGroup>,

    /// Channel to send stdin data
    pub stdin_tx: Option<mpsc::Sender<Vec<u8>>>,

    /// When the job was registered
    pub created_at: Instant,
}

impl JobEntry {
    /// Create a new job entry
    pub fn new() -> Self {
        Self {
            state: JobState::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            process_group: None,
            stdin_tx: None,
            created_at: Instant::now(),
        }
    }

    /// Get a clone of the cancellation flag
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl Default for JobEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn test_job_entry_new() {
        let entry = JobEntry::new();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancelled.load(Ordering::SeqCst));
        assert!(entry.process_group.is_none());
        assert!(entry.stdin_tx.is_none());
    }

    #[test]
    fn test_job_entry_default() {
        let entry: JobEntry = Default::default();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn test_job_entry_cancel_token() {
        let entry = JobEntry::new();
        let token1 = entry.cancel_token();
        let token2 = entry.cancel_token();

        token1.store(true, Ordering::SeqCst);
        assert!(token2.load(Ordering::SeqCst));
    }
}
