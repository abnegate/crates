use serde::{Deserialize, Serialize};

use super::Coverage;

/// A checkpoint of compacted history, kept apart from the history it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub content: String,
    pub coverage: Coverage,
    pub revision: u64,
}
