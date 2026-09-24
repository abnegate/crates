use serde::Deserialize;
use serde::Serialize;

/// A background job, as reported to a client and read back by the chat layer
/// from the spawn receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JobStarted {
    /// The job's id.
    pub id: String,
    /// The process id of the job's leader.
    pub pid: u32,
    /// Where the job's log is written.
    pub log_path: String,
}
