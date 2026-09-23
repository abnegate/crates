use serde::{Deserialize, Serialize};

use super::ContextSource;

/// A model's context limit and the output it reserves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub limit: Option<u64>,
    pub reserved: u32,
    pub source: ContextSource,
}

impl Policy {
    /// Preserve 20% of available input capacity for estimation uncertainty.
    pub fn threshold(&self) -> Option<u64> {
        self.input_limit()
            .map(|limit| limit.saturating_sub(limit / 5))
    }

    pub fn input_limit(&self) -> Option<u64> {
        self.limit
            .map(|limit| limit.saturating_sub(u64::from(self.reserved)))
    }
}
