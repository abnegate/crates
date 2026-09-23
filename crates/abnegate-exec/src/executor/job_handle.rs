use std::time::Duration;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

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

    /// When the child was spawned, which the job's timeout and reported
    /// duration count from
    pub started_at: Instant,

    /// Cancelling this stops the job: its process group is sent SIGTERM, then
    /// SIGKILL once the grace period has passed or the leader has exited, and
    /// a `RunError` with [`ErrorCode::Cancelled`](crate::protocol::ErrorCode)
    /// is reported. A job whose leader has already exited has its group
    /// killed anyway.
    pub cancellation: CancellationToken,
}

impl JobHandle {
    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Cancel the job
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Get elapsed time since start
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}
