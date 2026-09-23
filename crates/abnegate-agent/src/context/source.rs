use serde::Deserialize;
use serde::Serialize;

/// Where a context limit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContextSource {
    Runtime,
    Configured,
    Provider,
    Unknown,
}
