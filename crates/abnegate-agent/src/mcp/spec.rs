use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A stdio MCP server to launch and keep for an agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSpec {
    /// Registry key, and the prefix its tools are named under (`docs_search`).
    #[serde(default)]
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments after the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables overlaid on the inherited environment.
    ///
    /// Stdio MCP children are trusted local processes: they see `PATH`, `HOME`,
    /// and any credentials already in this process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory for the child. Inherits the process cwd when omitted.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Skip this server when true.
    #[serde(default)]
    pub disabled: bool,
}

impl McpServerSpec {
    /// A server launched as `command args…`, named `name`.
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: HashMap::new(),
            cwd: None,
            disabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_built_in_code_names_its_command_and_arguments() {
        let spec = McpServerSpec::new("magents", "magents", ["mcp"]);
        assert_eq!(spec.name, "magents");
        assert_eq!(spec.command, "magents");
        assert_eq!(spec.args, ["mcp"]);
        assert!(spec.env.is_empty());
        assert!(spec.cwd.is_none());
        assert!(!spec.disabled);
    }
}
