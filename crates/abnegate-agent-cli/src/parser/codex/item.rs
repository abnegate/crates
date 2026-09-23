use serde::Deserialize;

/// The part of a completed item this crate reads.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Item {
    #[serde(rename = "agent_message")]
    Message { text: String },
    #[serde(rename = "command_execution")]
    Command {
        #[serde(default)]
        id: String,
        #[serde(default)]
        command: String,
    },
    #[serde(other)]
    Ignored,
}
