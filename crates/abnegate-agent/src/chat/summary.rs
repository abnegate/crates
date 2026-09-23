use serde::Deserialize;
use serde::Serialize;

/// A stored checkpoint: its text, the entries it covers and their fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Summary {
    pub content: String,
    pub entries: Vec<String>,
    pub fingerprint: String,
    pub revision: u64,
}
