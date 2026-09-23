use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::tools::EnvironmentPolicy;

/// A stdio MCP server to launch and keep for an agent run.
///
/// Serialises in the Cursor `mcpServers` shape: `args`, `env` and `cwd` on the
/// wire, spelled out in full here.
///
/// The child is given the [`DEFAULT_ENVIRONMENT`](crate::tools::DEFAULT_ENVIRONMENT)
/// names from this process plus [`environment`](Self::environment), unless
/// [`inherit_environment`](Self::inherit_environment) opts in to the whole of
/// this process's environment. Values are [`SecretValue`]s, so printing a spec
/// never prints a key it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpServerSpec {
    /// Registry key, and the prefix its tools are named under (`docs__search`).
    #[serde(default)]
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments after the executable.
    #[serde(default, rename = "args", alias = "arguments")]
    pub arguments: Vec<String>,
    /// Variables the child is given on top of the allowlist.
    #[serde(default, rename = "env", alias = "environment")]
    pub environment: BTreeMap<String, SecretValue>,
    /// Give the child this process's whole environment, not the allowlist.
    #[serde(default)]
    pub inherit_environment: bool,
    /// Working directory for the child. Inherits the process's own when
    /// omitted.
    #[serde(default, rename = "cwd", alias = "working_directory")]
    pub working_directory: Option<PathBuf>,
    /// Skip this server when true.
    #[serde(default)]
    pub disabled: bool,
}

impl McpServerSpec {
    /// The same spec, giving its child `name` set to `value`.
    pub fn with_environment(
        mut self,
        name: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    /// The same spec, started in `working_directory`.
    pub fn within(mut self, working_directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(working_directory.into());
        self
    }

    /// The environment this spec's child is started with.
    pub fn environment_policy(&self) -> EnvironmentPolicy {
        let base = if self.inherit_environment {
            EnvironmentPolicy::inherit()
        } else {
            EnvironmentPolicy::allowlist()
        };
        self.environment.iter().fold(base, |policy, (name, value)| {
            policy.with(name, value.clone())
        })
    }

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
            environment: BTreeMap::new(),
            inherit_environment: false,
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
        assert!(!spec.inherit_environment);
        assert!(spec.working_directory.is_none());
        assert!(!spec.disabled);
    }

    #[test]
    fn debug_never_prints_an_environment_value() {
        let mut spec = McpServerSpec::new("notes", "notes-server", ["mcp"]);
        spec.environment
            .insert("API_TOKEN".to_string(), SecretValue::new("hunter2-secret"));
        let printed = format!("{spec:?}");
        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(!printed.contains("hunter2-secret"), "{printed}");
    }

    #[test]
    fn the_child_environment_is_the_allowlist_and_the_spec_unless_it_inherits() {
        let mut spec = McpServerSpec::new("notes", "notes-server", ["mcp"]);
        spec.environment
            .insert("NOTES_TOKEN".to_string(), SecretValue::new("token"));

        let policy = spec.environment_policy();
        assert!(!policy.inherits());
        assert_eq!(
            policy.get("NOTES_TOKEN").map(SecretValue::expose),
            Some("token")
        );
        for name in policy.names().filter(|name| *name != "NOTES_TOKEN") {
            assert!(crate::tools::DEFAULT_ENVIRONMENT.contains(&name), "{name}");
        }

        spec.inherit_environment = true;
        assert!(spec.environment_policy().inherits());
    }

    #[test]
    fn the_cursor_shape_reads_back_with_its_secrets_intact() {
        let spec: McpServerSpec = serde_json::from_value(serde_json::json!({
            "command": "notes-server",
            "args": ["mcp"],
            "env": {"NOTES_TOKEN": "token"},
            "cwd": "/srv/notes"
        }))
        .unwrap();
        assert_eq!(spec.arguments, ["mcp"]);
        assert_eq!(
            spec.environment.get("NOTES_TOKEN").map(SecretValue::expose),
            Some("token")
        );
        assert_eq!(spec.working_directory, Some(PathBuf::from("/srv/notes")));
        let written = serde_json::to_value(&spec).unwrap();
        assert_eq!(written["env"]["NOTES_TOKEN"], "token");
        assert_eq!(written["args"][0], "mcp");
    }
}
