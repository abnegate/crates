use serde::Deserialize;
use serde::Serialize;

/// A stored checkpoint: its text, the entries it covers and their fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    /// The checkpoint's text, standing in for the entries it covers.
    pub content: String,
    /// The ids of the entries it covers, in conversation order.
    pub entries: Vec<String>,
    /// The [`fingerprint`](super::fingerprint) of those entries when it was
    /// written.
    pub fingerprint: String,
    /// Rises with every checkpoint that replaces it.
    pub revision: u64,
}

impl Summary {
    /// Checkpoint `revision`, standing in for `entries` with `content`, over
    /// entries whose [`fingerprint`](super::fingerprint) was `fingerprint`.
    pub fn new(
        content: impl Into<String>,
        entries: Vec<String>,
        fingerprint: impl Into<String>,
        revision: u64,
    ) -> Self {
        Self {
            content: content.into(),
            entries,
            fingerprint: fingerprint.into(),
            revision,
        }
    }
}
