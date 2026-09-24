use serde::Deserialize;
use serde::Serialize;

/// A background job that is no longer running. No exit code means it was killed
/// rather than allowed to finish.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JobExited {
    /// The job's id.
    pub id: String,
    /// The code it exited with, when it was allowed to finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}
