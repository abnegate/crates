use serde::Deserialize;
use serde::Serialize;

/// Which entries a summary stands in for, and a fingerprint of exactly what
/// they held when it was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Coverage {
    /// The covered entries' ids, in history order.
    pub entries: Vec<String>,
    /// A SHA-256 over what the covered entries held, in hexadecimal.
    pub fingerprint: String,
}

impl Coverage {
    /// Coverage of `entries`, as [`coverage`](super::coverage) fingerprinted
    /// them.
    pub fn new(entries: Vec<String>, fingerprint: impl Into<String>) -> Self {
        Self {
            entries,
            fingerprint: fingerprint.into(),
        }
    }
}
