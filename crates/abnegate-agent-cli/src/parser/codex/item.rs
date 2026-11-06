use serde::Deserialize;
use serde_json::Value;

use crate::parser::codex::collab::Collab;
use crate::parser::codex::reason::Reason;
use crate::parser::lenient;

/// The part of an item this crate reads.
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
        /// Everything the command printed, stdout and stderr together.
        #[serde(default, deserialize_with = "lenient")]
        aggregated_output: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        exit_code: Option<i64>,
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
    /// A call to one of codex's collaboration tools.
    #[serde(rename = "collab_tool_call")]
    Collab(Collab),
    #[serde(other)]
    Ignored,
}
