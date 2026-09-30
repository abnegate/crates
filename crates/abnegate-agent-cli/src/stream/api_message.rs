use serde::Deserialize;

use crate::parser::claude::CliUsage;

/// The message a `message_start` event opens, without its content, which
/// the events after it stream.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ApiMessage {
    /// The message's identifier.
    #[serde(default)]
    pub id: Option<String>,
    /// The model answering.
    #[serde(default)]
    pub model: Option<String>,
    /// Why the model stopped, which is unset until a `message_delta` says.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// The prompt's token counts, known before any output is.
    #[serde(default)]
    pub usage: Option<CliUsage>,
}
