use serde::Deserialize;

use crate::wire::message::Message;

/// One completion choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    #[serde(default)]
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}
