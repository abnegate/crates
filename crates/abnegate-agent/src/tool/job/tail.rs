use super::JobStatus;

/// A slice of a job's log, and where the next one starts.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct JobTail {
    /// The slice of the log.
    pub output: String,
    /// Where the job stood when the slice was read.
    pub state: JobStatus,
    /// The log offset the next slice starts at.
    pub next: u64,
}
