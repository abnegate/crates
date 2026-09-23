use serde::Deserialize;
use serde::Serialize;

/// A background job, as reported to a client and read back by the chat layer
/// from the spawn receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStarted {
    pub id: String,
    pub pid: u32,
    pub log_path: String,
}
