use serde::Deserialize;

use crate::parser::lenient;

/// What a task claude runs has spent so far.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TaskUsage {
    #[serde(deserialize_with = "lenient")]
    pub(crate) total_tokens: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) tool_uses: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) duration_ms: Option<u64>,
}
