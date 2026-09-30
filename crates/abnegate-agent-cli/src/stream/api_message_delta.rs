use serde::Deserialize;

/// How a message ended, as a `message_delta` event reports it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ApiMessageDelta {
    /// Why the model stopped: `end_turn`, `tool_use`, `max_tokens`, and so on.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// The stop sequence that ended the message, when one did.
    #[serde(default)]
    pub stop_sequence: Option<String>,
}
