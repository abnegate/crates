use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use super::process_group::ProcessGroup;
use super::stdin_handle::StdinHandle;

/// Handle to a running job
#[derive(Debug)]
pub struct JobHandle {
    /// Process ID
    pub pid: u32,

    /// Process group for signaling
    pub process_group: ProcessGroup,

    /// Handle to send data to stdin
    pub stdin: Option<StdinHandle>,

    /// Start time
    pub started_at: Instant,

    /// Cancellation flag
    pub cancelled: Arc<AtomicBool>,
}

impl JobHandle {
    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Cancel the job
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Get elapsed time since start
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}
