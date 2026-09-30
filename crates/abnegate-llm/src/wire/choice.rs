use serde::Deserialize;

use crate::wire::message::Message;

/// One completion choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// Which answer this is, from 0.
    #[serde(default)]
    pub index: u32,
    /// The answer.
    pub message: Message,
    /// Why the model stopped, such as `stop` or `tool_calls`.
    pub finish_reason: Option<String>,
}
