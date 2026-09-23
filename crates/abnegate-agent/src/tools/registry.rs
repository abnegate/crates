use abnegate_llm::ToolDefinition;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use super::tail::TailJobTool;
use super::text::{MAX_PREVIEW_CHARACTERS, excerpt};
use super::{
    ApplyPatchTool, ListFilesTool, ReadFileTool, RunCommandTool, RunShellTool, SearchCodeTool,
    Tier, Tool, ToolContext, ToolError, ToolResult, WriteFileTool,
};

/// The tools an agent may call, by name.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Servers whose tools were attached over MCP, for prompt guidance.
    mcp_servers: Vec<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            mcp_servers: Vec::new(),
        }
    }

    /// The file tools, `run_command` and `tail_job`.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(ApplyPatchTool));
        registry.register(Arc::new(ListFilesTool));
        registry.register(Arc::new(SearchCodeTool));
        registry.register(Arc::new(RunCommandTool));
        registry.register(Arc::new(TailJobTool));
        registry
    }

    /// The default tools plus an unrestricted shell.
    ///
    /// Pair this with a [`ToolContext`] that has `unrestricted` set, or the
    /// file tools will still confine themselves to `cwd` while `run_shell`
    /// does not, which is the worst of both.
    pub fn with_host_tools() -> Self {
        let mut registry = Self::with_defaults();
        registry.register(Arc::new(RunShellTool));
        registry
    }

    /// Add a tool, replacing any already registered under its name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Every tool's definition, sorted by name.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|tool| tool.to_definition())
            .collect();
        // A stable order keeps the tools prefix identical across turns, so a
        // local server can reuse its prompt cache.
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));
        definitions
    }

    pub async fn execute(
        &self,
        name: &str,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.execute(parameters, context).await
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// A named tool's tier, or nothing when the catalog has no such tool.
    pub fn tier(&self, name: &str) -> Option<Tier> {
        self.tools.get(name).map(|tool| tool.tier())
    }

    /// Whether a named tool ends the turn, or nothing when the catalog has no
    /// such tool.
    pub fn ends_turn(&self, name: &str) -> Option<bool> {
        self.tools.get(name).map(|tool| tool.ends_turn())
    }

    /// Whether a named tool mutates state. Unknown names are treated as writes.
    pub fn mutating(&self, name: &str) -> bool {
        self.tier(name).is_none_or(Tier::mutating)
    }

    /// What a named call will do, bounded so one enormous argument cannot turn
    /// an approval card into a wall of text.
    pub fn preview(&self, name: &str, arguments: &str) -> Option<String> {
        let parameters: Value = serde_json::from_str(arguments).ok()?;
        let rendered = self.tools.get(name)?.preview(&parameters)?;
        Some(excerpt(&rendered, MAX_PREVIEW_CHARACTERS))
    }

    /// Take the tools out, for folding one registry into another.
    pub fn into_tools(self) -> Vec<Arc<dyn Tool>> {
        self.tools.into_values().collect()
    }

    /// Whether any MCP server's tools are registered.
    pub fn has_mcp(&self) -> bool {
        !self.mcp_servers.is_empty()
    }

    /// Record that a server's tools were attached, once per server.
    #[cfg(feature = "mcp")]
    pub(crate) fn attach(&mut self, server: String) {
        if !self.mcp_servers.contains(&server) {
            self.mcp_servers.push(server);
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::job::TAIL_JOB;
    use crate::tools::{LINE_BREAK, REASON_DESCRIPTION, REASON_PARAMETER};
    use std::collections::HashSet;

    /// A preview a reader approves has to say what will run. `sh -c` runs one
    /// command per line, so two lines joined by a space showed them a single
    /// command that was never going to run, and hid the one that was.
    #[test]
    fn a_multiline_shell_preview_keeps_the_commands_apart() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(RunShellTool));

        let preview = registry
            .preview(
                "run_shell",
                &serde_json::json!({"command": "cat config.toml\nrm -rf /srv/app"}).to_string(),
            )
            .expect("a shell call previews the line it will run");

        assert!(
            preview.contains(&format!("cat config.toml{LINE_BREAK}rm -rf /srv/app")),
            "the second command stays a second command: {preview}"
        );
    }

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new();
        assert!(registry.names().is_empty());
        assert!(!registry.has_mcp());
    }

    #[test]
    fn test_tool_registry_with_defaults() {
        let registry = ToolRegistry::with_defaults();
        let names = registry.names();

        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"search_code"));
        assert!(names.contains(&"run_command"));
        assert!(names.contains(&TAIL_JOB));
        assert_eq!(names.len(), 7);
    }

    /// `with_host_tools` is `with_defaults` plus a shell, so one registration
    /// is what puts a background job's log within reach of a chat and a task
    /// run alike, and registering it twice would be redundant.
    #[test]
    fn tail_job_is_registered_once_and_reaches_both_profiles() {
        let defaults = ToolRegistry::with_defaults();
        let host = ToolRegistry::with_host_tools();

        assert!(defaults.get(TAIL_JOB).is_some());
        assert!(host.get(TAIL_JOB).is_some());

        let mut added: Vec<&str> = host
            .names()
            .into_iter()
            .filter(|name| !defaults.names().contains(name))
            .collect();
        added.sort_unstable();
        assert_eq!(
            added,
            vec!["run_shell"],
            "the host profile adds only a shell"
        );
    }

    #[test]
    fn test_tool_registry_get() {
        let registry = ToolRegistry::with_defaults();

        assert!(registry.get("read_file").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_tool_registry_definitions() {
        let registry = ToolRegistry::with_defaults();
        let definitions = registry.definitions();

        assert_eq!(definitions.len(), 7);

        for definition in &definitions {
            assert_eq!(definition.tool_type, "function");
            assert!(!definition.function.name.is_empty());
            assert!(!definition.function.description.is_empty());
        }
    }

    #[tokio::test]
    async fn test_tool_registry_execute_not_found() {
        let registry = ToolRegistry::new();
        let context = ToolContext::default();

        let result = registry
            .execute("nonexistent", serde_json::json!({}), &context)
            .await;
        assert!(matches!(result, Err(ToolError::NotFound(_))));
    }

    fn side_effecting_tools() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(RunCommandTool),
            Arc::new(RunShellTool),
            Arc::new(WriteFileTool),
            Arc::new(ApplyPatchTool),
        ]
    }

    #[test]
    fn every_side_effecting_schema_lists_reason_as_a_property_and_as_required() {
        for tool in side_effecting_tools() {
            let schema = tool.parameters_schema();
            let property = &schema["properties"][REASON_PARAMETER];
            assert_eq!(property["type"], "string", "{}", tool.name());
            assert_eq!(
                property["description"],
                REASON_DESCRIPTION,
                "{}",
                tool.name()
            );

            let required = schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{} has no required array", tool.name()));
            assert!(
                required
                    .iter()
                    .any(|name| name.as_str() == Some(REASON_PARAMETER)),
                "{} does not require {REASON_PARAMETER}",
                tool.name()
            );
        }
    }

    #[test]
    fn one_reason_description_is_shared_by_every_side_effecting_schema() {
        let descriptions: HashSet<String> = side_effecting_tools()
            .iter()
            .map(|tool| {
                tool.parameters_schema()["properties"][REASON_PARAMETER]["description"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{} has no reason description", tool.name()))
                    .to_string()
            })
            .collect();

        assert_eq!(
            descriptions,
            HashSet::from([REASON_DESCRIPTION.to_string()]),
            "reason descriptions have forked"
        );
    }
}
