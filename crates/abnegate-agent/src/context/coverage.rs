use serde::Deserialize;
use serde::Serialize;

/// Which entries a summary stands in for, and a fingerprint of exactly what
/// they held when it was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub entries: Vec<String>,
    pub fingerprint: String,
}
