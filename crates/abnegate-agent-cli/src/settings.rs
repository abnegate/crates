//! How a spawned coding agent is run.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use abnegate_llm::Credential;
use abnegate_secret::SecretValue;

use crate::mcp::McpConfig;
use crate::mcp::McpServer;

/// Five minutes, matching the default for any command `abnegate-exec` runs.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Far more prose than any answer needs, and far less than a runaway agent
/// would otherwise hold in memory.
pub const DEFAULT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// One event is a JSON object holding at most a turn's worth of text.
pub const DEFAULT_LINE_LIMIT: usize = 1024 * 1024;

/// Claude Code's tools that read the workspace and the web but never change
/// anything, for a run that must leave the repository as it found it.
pub const READ_ONLY_TOOLS: [&str; 5] = ["Read", "Grep", "Glob", "WebFetch", "WebSearch"];

/// Claude Code's tools that change the workspace or run arbitrary commands,
/// which a read-only run denies outright.
pub const WRITE_TOOLS: [&str; 5] = ["Bash", "Edit", "MultiEdit", "Write", "NotebookEdit"];

/// How a [`CliProvider`](crate::CliProvider) runs its agent.
///
/// `Debug` is safe to log: the credential, every injected environment value
/// and every MCP secret print as redacted.
#[derive(Debug, Clone)]
pub struct CliSettings {
    /// Overrides the agent's own executable name. A relative name is resolved
    /// on `PATH` by the operating system.
    pub executable: Option<PathBuf>,
    /// The child's working directory. `None` inherits this process's.
    pub working_directory: Option<PathBuf>,
    pub credential: Credential,
    pub timeout: Duration,
    /// Bytes of the agent's prose, and of its diagnostics, kept. Prose past
    /// the limit abandons the run as malformed; diagnostics past it are still
    /// drained, and dropped. The stream around the prose is read to its end
    /// whatever its size, since none of it is kept.
    pub output_limit: usize,
    /// Bytes one event may occupy before the stream is treated as malformed.
    pub line_limit: usize,
    /// Set in the child's environment after the agent's
    /// [scrubbed](crate::AgentKind::scrubbed) variables are removed, so an
    /// explicit value always wins.
    pub environment: BTreeMap<String, SecretValue>,
    /// Extra flags passed through verbatim, after the streaming flags and
    /// before the model. Nothing here is checked against the agent.
    pub arguments: Vec<String>,
    /// A JSON schema the final answer must satisfy. Claude only.
    pub schema: Option<String>,
    /// Appended to the agent's own system prompt. Claude only.
    pub instructions: Option<String>,
    /// Tools the agent may use without asking. Claude only.
    pub permissions: Vec<String>,
    /// Deny [`WRITE_TOOLS`] and refuse any flag that bypasses permission
    /// prompts, so the run cannot change the workspace however it is asked
    /// to. Claude only.
    pub read_only: bool,
    /// MCP servers to attach, whose tools are allowed alongside
    /// `permissions`. Claude only.
    pub mcp: McpConfig,
    /// Where each run keeps its [execution logs](crate::log). `None` keeps none.
    pub log: Option<PathBuf>,
    /// A stderr line this returns true for settles the run as failed, in the
    /// line's own words, and the agent is stopped: an agent retrying against
    /// a rate limit is stopped instead of waited on until the timeout. Stdout
    /// is never checked, since the agent's prose can quote anything.
    pub tripwire: Option<fn(&str) -> bool>,
}

impl Default for CliSettings {
    fn default() -> Self {
        Self {
            executable: None,
            working_directory: None,
            credential: Credential::Inherited,
            timeout: DEFAULT_TIMEOUT,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            line_limit: DEFAULT_LINE_LIMIT,
            environment: BTreeMap::new(),
            arguments: Vec::new(),
            schema: None,
            instructions: None,
            permissions: Vec::new(),
            read_only: false,
            mcp: McpConfig::default(),
            log: None,
            tripwire: None,
        }
    }
}

impl CliSettings {
    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = Some(executable.into());
        self
    }

    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    pub fn with_credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_output_limit(mut self, limit: usize) -> Self {
        self.output_limit = limit;
        self
    }

    pub fn with_line_limit(mut self, limit: usize) -> Self {
        self.line_limit = limit;
        self
    }

    pub fn with_environment(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.environment.insert(variable.into(), value.into());
        self
    }

    pub fn with_arguments<I>(mut self, arguments: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn with_permissions<I>(mut self, permissions: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.permissions
            .extend(permissions.into_iter().map(Into::into));
        self
    }

    /// Allow exactly [`READ_ONLY_TOOLS`], replacing any permission set so
    /// far, and hold the run to [`CliSettings::read_only`].
    pub fn read_only(mut self) -> Self {
        self.permissions = READ_ONLY_TOOLS.map(str::to_string).to_vec();
        self.read_only = true;
        self
    }

    pub fn with_mcp_server(mut self, name: impl Into<String>, server: McpServer) -> Self {
        self.mcp = self.mcp.with_server(name, server);
        self
    }

    pub fn with_log(mut self, root: impl Into<PathBuf>) -> Self {
        self.log = Some(root.into());
        self
    }

    pub fn with_tripwire(mut self, tripwire: fn(&str) -> bool) -> Self {
        self.tripwire = Some(tripwire);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;

    use super::CliSettings;
    use super::DEFAULT_LINE_LIMIT;
    use super::DEFAULT_OUTPUT_LIMIT;
    use super::DEFAULT_TIMEOUT;
    use super::READ_ONLY_TOOLS;
    use crate::mcp::McpServer;

    #[test]
    fn defaults_inherit_the_hosts_session_and_directory() {
        let settings = CliSettings::default();

        assert!(matches!(settings.credential, Credential::Inherited));
        assert!(settings.executable.is_none());
        assert!(settings.working_directory.is_none());
        assert_eq!(settings.timeout, DEFAULT_TIMEOUT);
        assert!(settings.environment.is_empty());
        assert!(settings.arguments.is_empty());
        assert!(settings.schema.is_none());
        assert!(settings.instructions.is_none());
        assert!(settings.permissions.is_empty());
        assert!(!settings.read_only);
        assert!(settings.mcp.is_empty());
        assert!(settings.log.is_none());
        assert!(settings.tripwire.is_none());
    }

    #[test]
    fn debug_never_prints_the_credential() {
        let settings = CliSettings::default()
            .with_credential(Credential::key("ANTHROPIC_API_KEY", "sk-ant-notarealkey"));

        let rendered = format!("{settings:?}");
        assert!(
            !rendered.contains("sk-ant"),
            "credential leaked: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn debug_never_prints_an_injected_or_mcp_secret() {
        let settings = CliSettings::default()
            .with_environment("GITHUB_TOKEN", "ghp_notarealtoken")
            .with_mcp_server(
                "grafana",
                McpServer {
                    command: Some("uvx".to_string()),
                    environment: [(
                        "GRAFANA_TOKEN".to_string(),
                        SecretValue::new("glsa_realsecret"),
                    )]
                    .into(),
                    ..McpServer::default()
                },
            );

        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("ghp_notarealtoken"), "{rendered}");
        assert!(!rendered.contains("glsa_realsecret"), "{rendered}");
        assert!(rendered.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn builders_replace_only_what_they_name() {
        let settings = CliSettings::default()
            .with_executable("/opt/bin/claude")
            .with_timeout(Duration::from_secs(30));

        assert_eq!(settings.executable, Some(PathBuf::from("/opt/bin/claude")));
        assert_eq!(settings.timeout, Duration::from_secs(30));
        assert_eq!(settings.output_limit, DEFAULT_OUTPUT_LIMIT);
        assert_eq!(settings.line_limit, DEFAULT_LINE_LIMIT);
    }

    #[test]
    fn list_builders_accumulate() {
        let settings = CliSettings::default()
            .with_arguments(["--json-schema", "{}"])
            .with_arguments(vec!["--verbose".to_string()])
            .with_permissions(["Read"])
            .with_permissions(["Edit"])
            .with_output_limit(1024)
            .with_line_limit(256)
            .with_schema("{}")
            .with_instructions("Be terse.")
            .with_working_directory("/w")
            .with_log("/var/log/agents");

        assert_eq!(settings.arguments, ["--json-schema", "{}", "--verbose"]);
        assert_eq!(settings.permissions, ["Read", "Edit"]);
        assert_eq!(settings.output_limit, 1024);
        assert_eq!(settings.line_limit, 256);
        assert_eq!(settings.schema.as_deref(), Some("{}"));
        assert_eq!(settings.instructions.as_deref(), Some("Be terse."));
        assert_eq!(settings.working_directory, Some(PathBuf::from("/w")));
        assert_eq!(settings.log, Some(PathBuf::from("/var/log/agents")));
    }

    #[test]
    fn a_tripwire_is_kept_as_given() {
        let settings = CliSettings::default().with_tripwire(|line| line.contains("429"));
        let tripwire = settings.tripwire.expect("a tripwire");

        assert!(tripwire("HTTP 429 Too Many Requests"));
        assert!(!tripwire("compiling"));
    }

    #[test]
    fn a_read_only_run_allows_exactly_the_read_only_tools() {
        let settings = CliSettings::default()
            .with_permissions(["Bash", "Edit"])
            .read_only();

        assert!(settings.read_only);
        assert_eq!(settings.permissions, READ_ONLY_TOOLS);
        assert_eq!(
            READ_ONLY_TOOLS,
            ["Read", "Grep", "Glob", "WebFetch", "WebSearch"]
        );
    }
}
