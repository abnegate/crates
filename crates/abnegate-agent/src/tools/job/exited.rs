use serde::{Deserialize, Serialize};

/// A background job that is no longer running. No exit code means it was killed
/// rather than allowed to finish.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobExited {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}
