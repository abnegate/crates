use serde::{Deserialize, Serialize};

/// Estimated input tokens, by what spent them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub instructions: u64,
    pub conversation: u64,
    pub tools: u64,
    pub results: u64,
    pub summary: u64,
    pub attachments: Option<u64>,
    pub overhead: u64,
}

impl ContextBreakdown {
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
