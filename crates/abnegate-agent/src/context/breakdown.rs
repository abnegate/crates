use serde::Deserialize;
use serde::Serialize;

/// Estimated input tokens, by what spent them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextBreakdown {
    /// System messages.
    pub instructions: u64,
    /// User and assistant messages the checkpoint does not cover.
    pub conversation: u64,
    /// The tool definitions offered to the model.
    pub tools: u64,
    /// Tool output the checkpoint does not cover.
    pub results: u64,
    /// The checkpoint standing in for compacted history.
    pub summary: u64,
    /// Images, or `None` when some are present and cannot be estimated, which
    /// makes [`total`](Self::total) a lower bound.
    pub attachments: Option<u64>,
    /// Message and request framing, names, tool call arguments and reasoning.
    pub overhead: u64,
}

impl ContextBreakdown {
    /// Every category added up, saturating rather than overflowing.
    pub fn total(&self) -> u64 {
        [
            self.instructions,
            self.conversation,
            self.tools,
            self.results,
            self.summary,
            self.attachments.unwrap_or_default(),
            self.overhead,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}
