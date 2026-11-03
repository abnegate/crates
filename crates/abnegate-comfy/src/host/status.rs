use serde::Deserialize;
use serde::Serialize;

/// Where a host [`Job`](crate::host::Job) stands, as `job.json` names it in
/// its `status` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
    /// Written and waiting for the host trainer to start it.
    Queued,
    /// Started by the host trainer.
    Running,
    /// Finished, with the weight written into the job directory.
    Succeeded,
    /// Given up on, with the reason in the job's `error`.
    Failed,
}

impl Status {
    /// The name the status travels under in `job.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Whether the job is still waiting for, or holding, the host trainer.
    pub fn busy(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}
