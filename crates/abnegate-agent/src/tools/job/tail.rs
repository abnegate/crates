use super::JobState;

/// A slice of a job's log, and where the next one starts.
#[derive(Debug, Clone, PartialEq)]
pub struct JobTail {
    pub output: String,
    pub state: JobState,
    pub next: u64,
}
