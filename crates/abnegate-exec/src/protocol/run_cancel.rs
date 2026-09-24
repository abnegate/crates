use serde::Deserialize;
use serde::Serialize;

/// Stops a running command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunCancel {
    /// The job to stop
    pub job_id: String,
    /// Send SIGKILL at once instead of SIGTERM followed by the grace period
    #[serde(default)]
    pub force: bool,
}

impl RunCancel {
    /// Stop `job_id` with SIGTERM, then SIGKILL once the grace period has
    /// passed.
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            force: false,
        }
    }

    /// Whether to send SIGKILL at once.
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}
