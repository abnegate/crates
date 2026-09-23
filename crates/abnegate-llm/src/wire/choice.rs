use serde::Deserialize;

use crate::wire::message::Message;

/// One completion choice.
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}
