use serde::Deserialize;
use serde::Serialize;

use super::Coverage;

/// A checkpoint of compacted history, kept apart from the history it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Summary {
    /// The structured state the summarizer wrote.
    pub content: String,
    /// The entries it stands in for, and what they held.
    pub coverage: Coverage,
    /// Counts up from 1 each time the checkpoint is rewritten.
    pub revision: u64,
}

impl Summary {
    /// Revision `revision` of a checkpoint holding `content` in place of what
    /// `coverage` names.
    pub fn new(content: impl Into<String>, coverage: Coverage, revision: u64) -> Self {
        Self {
            content: content.into(),
            coverage,
            revision,
        }
    }
}
