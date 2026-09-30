use serde::Deserialize;
use serde_json::Value;

use crate::parser::codex::reason::Reason;

/// The part of a completed item this crate reads.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum Item {
    #[serde(rename = "agent_message")]
    Message { text: String },
    #[serde(rename = "command_execution")]
    Command {
        #[serde(default)]
        id: String,
        #[serde(default)]
        command: String,
    },
    /// A call to an MCP server's tool, completed or refused. Codex's own
    /// refusal carries `error`; a server's refusal is its `result`.
    #[serde(rename = "mcp_tool_call")]
    Call {
        #[serde(default)]
        id: String,
        server: String,
        tool: String,
        #[serde(default)]
        arguments: Option<Value>,
        #[serde(default)]
        error: Option<Reason>,
    },
    #[serde(other)]
    Ignored,
}
