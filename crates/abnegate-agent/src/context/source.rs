use serde::{Deserialize, Serialize};

/// Where a context limit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    Runtime,
    Configured,
    Provider,
    Unknown,
}
