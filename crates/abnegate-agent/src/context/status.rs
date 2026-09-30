use serde::Deserialize;
use serde::Serialize;

/// Where a conversation stands against its context limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextStatus {
    /// Within the limit; nothing needed doing.
    Ready,
    /// Compaction is under way, for a caller reporting progress.
    Compacting,
    /// Older history was folded into a new checkpoint to fit.
    Compacted,
    /// No limit is known, so nothing could be judged.
    Unavailable,
    /// Compaction failed or freed too little, and the history went out
    /// uncompacted because it still fits the input limit.
    Blocked,
}
