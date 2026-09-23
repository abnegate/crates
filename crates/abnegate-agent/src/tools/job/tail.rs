use super::JobStatus;

/// A slice of a job's log, and where the next one starts.
#[derive(Debug, Clone, PartialEq)]
pub struct JobTail {
    pub output: String,
    pub state: JobStatus,
    pub next: u64,
}
