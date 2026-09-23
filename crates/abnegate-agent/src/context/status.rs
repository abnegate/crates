use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextStatus {
    Ready,
    Compacting,
    Compacted,
    Unavailable,
    Blocked,
}
