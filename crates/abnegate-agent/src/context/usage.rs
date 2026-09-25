use serde::Deserialize;
use serde::Serialize;

use super::ContextBreakdown;
use super::ContextSource;
use super::ContextStatus;

/// How much of a model's context a request spends, and what that leaves.
///
/// [`estimate`](super::estimate) and [`prepare`](super::prepare) report one.
/// Outside this crate one is built from [`Default`], a fixture's for
/// instance, and the fields it pins are set on it:
///
/// ```
/// use abnegate_agent::context::ContextSource;
/// use abnegate_agent::context::ContextStatus;
/// use abnegate_agent::context::ContextUsage;
///
/// let mut usage = ContextUsage::default();
/// usage.model = "qwen3".to_string();
/// usage.used = 3_064;
/// usage.limit = Some(32_768);
/// usage.source = ContextSource::Configured;
/// usage.status = ContextStatus::Ready;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextUsage {
    /// The model the request is for.
    pub model: String,
    /// Estimated input tokens the request spends.
    pub used: u64,
    /// The model's whole context in tokens, when it is known.
    pub limit: Option<u64>,
    /// Tokens held back from the input for the reply.
    pub reserved: u32,
    /// Input tokens a request may spend before history is compacted, when
    /// the limit is known.
    pub threshold: Option<u64>,
    /// Input tokens left under the threshold, when there is one.
    pub remaining: Option<u64>,
    /// Whether the counts are estimated rather than measured by the model's
    /// own tokenizer.
    pub estimated: bool,
    /// Whether part of the request, an image, could not be priced, so that
    /// `used` counts only what could.
    pub incomplete: bool,
    /// Where `limit` came from.
    pub source: ContextSource,
    /// Where compaction stands.
    pub status: ContextStatus,
    /// `used`, by what spent it.
    pub breakdown: ContextBreakdown,
    /// The revision of the checkpoint the request is sent through, or 0
    /// without one.
    pub revision: u64,
    /// How many history entries the checkpoint stands in for.
    pub compacted_messages: usize,
    /// When the usage was reported, as RFC 3339.
    pub updated_at: String,
    /// Why the status is what it is, when that needs saying.
    pub reason: Option<String>,
}

/// Nothing spent against no known limit, read the way
/// [`estimate`](super::estimate) reads a request with none: every count
/// zero, nothing named, the limit's [source](ContextSource::Unknown) unknown
/// and compaction [`Unavailable`](ContextStatus::Unavailable).
impl Default for ContextUsage {
    fn default() -> Self {
        Self {
            model: String::new(),
            used: 0,
            limit: None,
            reserved: 0,
            threshold: None,
            remaining: None,
            estimated: false,
            incomplete: false,
            source: ContextSource::Unknown,
            status: ContextStatus::Unavailable,
            breakdown: ContextBreakdown::default(),
            revision: 0,
            compacted_messages: 0,
            updated_at: String::new(),
            reason: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Policy;
    use crate::context::estimate;

    #[test]
    fn the_default_spends_nothing_against_no_known_limit() {
        let usage = ContextUsage::default();

        assert_eq!(usage.model, "");
        assert_eq!((usage.used, usage.reserved), (0, 0));
        assert_eq!(
            (usage.limit, usage.threshold, usage.remaining),
            (None, None, None)
        );
        assert!(!usage.estimated);
        assert!(!usage.incomplete);
        assert_eq!(usage.breakdown, ContextBreakdown::default());
        assert_eq!((usage.revision, usage.compacted_messages), (0, 0));
        assert_eq!(usage.updated_at, "");
        assert_eq!(usage.reason, None);

        let unlimited = estimate(
            "",
            &[],
            None,
            &Policy::new(None, 0, ContextSource::Unknown),
            None,
        );
        assert_eq!(
            (usage.source, usage.status),
            (unlimited.source, unlimited.status),
            "a usage with no limit reads the way an estimate with none does"
        );
    }
}
