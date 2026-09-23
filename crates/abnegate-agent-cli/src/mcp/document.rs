use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::mcp::server::McpServer;

/// An MCP configuration as a file holds it: the servers under the CLI's
/// own `mcpServers` key, or a bare map of them.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Document {
    Wrapped {
        #[serde(rename = "mcpServers")]
        servers: BTreeMap<String, McpServer>,
    },
    Bare(BTreeMap<String, McpServer>),
}
