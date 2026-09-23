use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A stdio MCP server to launch and keep for an agent run.
///
/// Serialises in the Cursor `mcpServers` shape: `args`, `env` and `cwd` on the
/// wire, spelled out in full here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSpec {
    /// Registry key, and the prefix its tools are named under (`docs__search`).
    #[serde(default)]
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments after the executable.
    #[serde(default, rename = "args", alias = "arguments")]
    pub arguments: Vec<String>,
    /// Extra environment variables overlaid on the inherited environment.
    #[serde(default, rename = "env", alias = "environment")]
    pub environment: HashMap<String, String>,
    /// Working directory for the child. Inherits the process's own when
    /// omitted.
    #[serde(default, rename = "cwd", alias = "working_directory")]
    pub working_directory: Option<PathBuf>,
    /// Skip this server when true.
    #[serde(default)]
    pub disabled: bool,
}

impl McpServerSpec {
    /// A server launched as `command arguments…`, named `name`.
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            environment: HashMap::new(),
            working_directory: None,
            disabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_built_in_code_names_its_command_and_arguments() {
        let spec = McpServerSpec::new("notes", "notes-server", ["mcp"]);
        assert_eq!(spec.name, "notes");
        assert_eq!(spec.command, "notes-server");
        assert_eq!(spec.arguments, ["mcp"]);
        assert!(spec.environment.is_empty());
        assert!(spec.working_directory.is_none());
        assert!(!spec.disabled);
    }
}
