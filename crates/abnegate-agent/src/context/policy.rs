use serde::Deserialize;
use serde::Serialize;

use super::ContextSource;

/// A model's context limit and the output it reserves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Policy {
    /// The model's whole context in tokens, when it is known.
    pub limit: Option<u64>,
    /// Tokens held back from the input for the reply.
    pub reserved: u32,
    /// Where `limit` came from.
    pub source: ContextSource,
}

impl Policy {
    /// A context of `limit` tokens, when known, that `source` reported,
    /// holding `reserved` of them back for the reply.
    pub fn new(limit: Option<u64>, reserved: u32, source: ContextSource) -> Self {
        Self {
            limit,
            reserved,
            source,
        }
    }

    /// Preserve 20% of available input capacity for estimation uncertainty.
    pub fn threshold(&self) -> Option<u64> {
        self.input_limit()
            .map(|limit| limit.saturating_sub(limit / 5))
    }

    /// The context left for input once the reply's reservation is held back.
    pub fn input_limit(&self) -> Option<u64> {
        self.limit
            .map(|limit| limit.saturating_sub(u64::from(self.reserved)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_carries_what_it_was_given() {
        let policy = Policy::new(Some(5_000), 1_024, ContextSource::Configured);

        assert_eq!(policy.limit, Some(5_000));
        assert_eq!(policy.reserved, 1_024);
        assert_eq!(policy.source, ContextSource::Configured);
        assert_eq!(policy.input_limit(), Some(3_976));
    }
}
