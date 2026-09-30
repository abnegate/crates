use std::fmt;

use super::JobExited;

/// Where a job has got to, spelled the way `tail_job` reports it.
///
/// Not `abnegate_exec::JobState`: that one tracks an executor run through
/// its lifecycle, this one is what a model is told about a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JobStatus {
    /// Still running.
    Running,
    /// Exited on its own with this code.
    Exited(i32),
    /// Stopped without an exit code: cancelled, past
    /// [`MAXIMUM_JOB_LIFETIME`](super::MAXIMUM_JOB_LIFETIME), or ended by a
    /// signal.
    Killed,
    /// Killed for writing more than
    /// [`MAXIMUM_JOB_LOG_BYTES`](super::MAXIMUM_JOB_LOG_BYTES) of log.
    Flooded,
}

impl fmt::Display for JobStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => formatter.write_str("running"),
            Self::Exited(code) => write!(formatter, "exited {code}"),
            Self::Killed => formatter.write_str("killed"),
            Self::Flooded => formatter.write_str("flooded"),
        }
    }
}

impl JobStatus {
    /// Whether the job has stopped, one way or another.
    pub fn settled(self) -> bool {
        !matches!(self, Self::Running)
    }

    /// A job that stopped without an exit code was stopped by us.
    pub(super) fn exited(self, id: String) -> JobExited {
        JobExited {
            id,
            exit_code: match self {
                Self::Exited(code) => Some(code),
                _ => None,
            },
        }
    }
}
