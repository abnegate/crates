use serde::Deserialize;
use serde::Serialize;

/// A page of an evidence record or of the evidence catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub content: String,
    pub offset: u64,
    pub next: Option<u64>,
    pub total: u64,
}
