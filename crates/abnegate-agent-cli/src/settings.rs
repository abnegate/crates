//! How a spawned coding agent is run.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use abnegate_llm::Credential;
use abnegate_secret::SecretValue;

use crate::mcp::McpConfig;
use crate::mcp::McpServer;
use crate::tripwire::Tripwire;

/// Five minutes, matching the default for any command `abnegate-exec` runs.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Far more prose than any answer needs, and far less than a runaway agent
/// would otherwise hold in memory.
pub const DEFAULT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// One event is a JSON object holding at most a turn's worth of text.
pub const DEFAULT_LINE_LIMIT: usize = 1024 * 1024;

/// Room for every line of a long session, and a bound on a runaway one's
/// share of the disk.
pub const DEFAULT_JOURNAL_LIMIT: u64 = 64 * 1024 * 1024;

/// The proxy variables [`CliSettings::with_proxy_variables`] allows, in
/// both the upper and the lower case tools read them in.
const PROXY_VARIABLES: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// Claude Code's tools that read files but never change anything, and the
/// only built-in tools a read-only run makes available unless it
/// [reaches the web](CliSettings::web). A read-only run allows them only
/// inside its working directory.
pub const READ_ONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob"];

/// Claude Code's tools that reach the network: a fetch of any URL, and a web
/// search. Only [`CliSettings::web`] allows them.
pub const WEB_TOOLS: &[&str] = &["WebFetch", "WebSearch"];

/// Caller [arguments](CliSettings::arguments) a read-only run passes through
/// that take no value. Only the long form is recognised.
pub const READ_ONLY_SWITCHES: &[&str] = &[
    "--exclude-dynamic-system-prompt-sections",
    "--fork-session",
    "--include-partial-messages",
    "--no-session-persistence",
];

/// Caller [arguments](CliSettings::arguments) a read-only run passes through
/// that take one value, given either as the next argument or after `=`.
pub const READ_ONLY_OPTIONS: &[&str] = &[
    "--effort",
    "--fallback-model",
    "--max-budget-usd",
    "--name",
    "--resume",
    "--session-id",
];

/// How a [`CliProvider`](crate::CliProvider) runs its agent.
///
/// `Debug` is safe to log: the credential, every injected environment value
/// and every MCP secret print as redacted.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// Bytes one event may occupy. A longer one is dropped and counted in
    /// [`StdoutParseResult::dropped`](crate::StdoutParseResult::dropped),
    /// unless it is the result or prose the run cannot do without, which
    /// makes the stream malformed.
    pub line_limit: usize,
    /// Set in the child's environment on top of what it is given from the
    /// host, so an explicit value always wins. Every value is treated as a
    /// secret and scrubbed from whatever the run writes down, as written,
    /// JSON-escaped or percent-encoded; one the agent re-encodes any other
    /// way, such as in base64, is not recognised. A setting that is not
    /// secret belongs in `variables`.
    pub environment: BTreeMap<String, SecretValue>,
    /// Set in the child's environment like `environment`, which wins over
    /// them, but never scrubbed: flags such as `DISABLE_AUTOUPDATER=1`,
    /// whose values would otherwise be redacted wherever they appear.
    pub variables: BTreeMap<String, String>,
    /// Host variables the child is given on top of
    /// [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT), each with
    /// this process's own value and only when this process has it set.
    ///
    /// Empty by default: a proxy URL can carry credentials, so a proxy
    /// reaches the child only through [`CliSettings::with_proxy_variables`],
    /// a name [allowed](CliSettings::allow) here, or
    /// [`CliSettings::inherit_environment`]. Every value passed this way but
    /// the proxy bypass list, `NO_PROXY` and `no_proxy`, is scrubbed from
    /// what the run writes down, like one set in
    /// [`environment`](CliSettings::environment). The agent's
    /// [nested-session marker](crate::AgentKind::scrubbed) is never passed
    /// this way: a run that must see it sets it explicitly.
    pub allowed: BTreeSet<String>,
    /// Give the child the host's whole environment, less the agent's
    /// [scrubbed](crate::AgentKind::scrubbed) variables, instead of
    /// [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) and the
    /// [`allowed`](CliSettings::allowed) names alone.
    ///
    /// Off by default: the agent runs tools the model chooses, and anything
    /// in its environment is theirs to read. Without it the child is also
    /// given the agent's own [configuration](crate::AgentKind::configuration)
    /// variables and its [sign-in](crate::AgentKind::credentials) variables
    /// when the credential is [inherited](Credential::Inherited). An attached
    /// MCP server's values reach it under generated names either way: see
    /// [`McpAttachment`](crate::McpAttachment). Anything else it needs, such
    /// as a proxy, is [allowed](CliSettings::allow) or set explicitly.
    pub inherit_environment: bool,
    /// Extra flags passed through verbatim, after the streaming flags and
    /// before the model. Nothing here is checked against the agent, except
    /// in a [read-only](CliSettings::read_only) run, which refuses anything
    /// not in [`READ_ONLY_SWITCHES`] or [`READ_ONLY_OPTIONS`].
    pub arguments: Vec<String>,
    /// A JSON schema the final answer must satisfy. Claude only.
    pub schema: Option<String>,
    /// Appended to the agent's own system prompt, through a private
    /// temporary file rather than the command line, whose size is limited.
    /// Claude only.
    pub instructions: Option<String>,
    /// Tools the agent may use without asking. Claude only.
    pub permissions: Vec<String>,
    /// Confine the run to an allowlist, so it cannot change the workspace or
    /// read outside it however it is asked to. Claude only.
    ///
    /// Only the [`READ_ONLY_TOOLS`] named in `permissions` are available at
    /// all, and they are allowed only inside the working directory, which the
    /// CLI's restricted mode also holds every file tool to; the
    /// [`WEB_TOOLS`] are available only with [`CliSettings::web`]; anything
    /// that would need permission is denied rather than asked about; no
    /// settings file loads, the user's included, so a repository cannot add
    /// hooks, permissions or plugins of its own, and managed settings alone
    /// apply; only the MCP servers attached here load, and only the tools
    /// each one names are allowed, never a whole server; and every caller
    /// argument outside [`READ_ONLY_SWITCHES`] and [`READ_ONLY_OPTIONS`] is
    /// refused. A permission that is neither a read-only tool nor one named
    /// MCP tool is refused too, and so is a web tool without
    /// [`CliSettings::web`].
    ///
    /// Anything the agent would otherwise read from the user's settings, such
    /// as a provider or proxy variable, is passed in `variables`,
    /// `environment` or `allowed` instead.
    pub read_only: bool,
    /// Allow the [`WEB_TOOLS`] without asking, and make them available to a
    /// [read-only](CliSettings::read_only) run, which otherwise has neither.
    /// Claude only.
    ///
    /// Off by default: either tool can carry whatever the agent has read to
    /// any address, and a prompt injected through the workspace, or through
    /// a page fetched along the way, chooses the address.
    pub web: bool,
    /// MCP servers to attach, whose tools are allowed alongside
    /// `permissions`. Claude only.
    pub mcp: McpConfig,
    /// Where each run keeps its [execution logs](crate::log). `None` keeps none.
    pub log: Option<PathBuf>,
    /// Bytes of a run's journal the lines it printed may fill, past which
    /// they are no longer recorded.
    pub journal_limit: u64,
    /// A stderr line this returns true for settles the run as failed, in the
    /// line's own words, and the agent is stopped: an agent retrying against
    /// a rate limit is stopped instead of waited on until the timeout. Stdout
    /// is never checked, since the agent's prose can quote anything.
    pub tripwire: Option<Tripwire>,
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
            variables: BTreeMap::new(),
            allowed: BTreeSet::new(),
            inherit_environment: false,
            arguments: Vec::new(),
            schema: None,
            instructions: None,
            permissions: Vec::new(),
            read_only: false,
            web: false,
            mcp: McpConfig::default(),
            log: None,
            journal_limit: DEFAULT_JOURNAL_LIMIT,
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

    /// Set `variable` to `value` in the child's environment, as a secret
    /// the run scrubs from whatever it writes down. See
    /// [`CliSettings::environment`].
    pub fn with_environment(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.environment.insert(variable.into(), value.into());
        self
    }

    pub fn with_variable(mut self, variable: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(variable.into(), value.into());
        self
    }

    /// Also give the child each of `names` from this process's environment,
    /// when this process has it set. See [`CliSettings::allowed`].
    ///
    /// Each allowed value but the proxy bypass list is scrubbed from what the
    /// run writes down wherever it appears, as a secret set with
    /// [`CliSettings::with_environment`] is, so allowing a variable whose
    /// value is ordinary text, such as a URL a log would name, hides that
    /// text from every log of the run. A setting that is not secret and must
    /// stay legible belongs in [`CliSettings::with_variable`].
    pub fn allow<I>(mut self, names: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.allowed.extend(names.into_iter().map(Into::into));
        self
    }

    /// Also give the child this process's proxy settings: `HTTP_PROXY`,
    /// `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`, in upper and lower case,
    /// each when this process has it set.
    ///
    /// For an agent that reaches its API through a proxy. Off by default,
    /// because a proxy URL can carry credentials; each proxy URL is scrubbed
    /// from what the run writes down, as every [allowed](CliSettings::allow)
    /// value but the bypass list is.
    pub fn with_proxy_variables(self) -> Self {
        self.allow(PROXY_VARIABLES.iter().copied())
    }

    /// Opt in to [`CliSettings::inherit_environment`].
    pub fn inherit_environment(mut self) -> Self {
        self.inherit_environment = true;
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
    /// far, and hold the run to [`CliSettings::read_only`]. Named MCP tools
    /// may be allowed afterwards with [`CliSettings::with_permissions`], and
    /// the web, before or after, with [`CliSettings::allow_web`].
    pub fn read_only(mut self) -> Self {
        self.permissions = READ_ONLY_TOOLS.iter().copied().map(String::from).collect();
        self.read_only = true;
        self
    }

    /// Opt in to [`CliSettings::web`].
    pub fn allow_web(mut self) -> Self {
        self.web = true;
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

    pub fn with_journal_limit(mut self, limit: u64) -> Self {
        self.journal_limit = limit;
        self
    }

    pub fn with_tripwire(
        mut self,
        tripwire: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.tripwire = Some(Tripwire::new(tripwire));
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
    use super::DEFAULT_JOURNAL_LIMIT;
    use super::DEFAULT_LINE_LIMIT;
    use super::DEFAULT_OUTPUT_LIMIT;
    use super::DEFAULT_TIMEOUT;
    use super::READ_ONLY_TOOLS;
    use super::WEB_TOOLS;
    use crate::mcp::McpServer;

    #[test]
    fn defaults_inherit_the_hosts_session_and_directory() {
        let settings = CliSettings::default();

        assert!(matches!(settings.credential, Credential::Inherited));
        assert!(settings.executable.is_none());
        assert!(settings.working_directory.is_none());
        assert_eq!(settings.timeout, DEFAULT_TIMEOUT);
        assert!(settings.environment.is_empty());
        assert!(settings.variables.is_empty());
        assert!(settings.allowed.is_empty());
        assert!(!settings.inherit_environment);
        assert!(settings.arguments.is_empty());
        assert!(settings.schema.is_none());
        assert!(settings.instructions.is_none());
        assert!(settings.permissions.is_empty());
        assert!(!settings.read_only);
        assert!(!settings.web);
        assert!(settings.mcp.is_empty());
        assert!(settings.log.is_none());
        assert_eq!(settings.journal_limit, DEFAULT_JOURNAL_LIMIT);
        assert!(settings.tripwire.is_none());
    }

    #[test]
    fn debug_never_prints_the_credential() {
        let settings = CliSettings::default().with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "notarealkey"),
        ));

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
            .with_environment("GITHUB_TOKEN", concat!("ghp_", "notarealtoken"))
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
        assert!(
            !rendered.contains(concat!("ghp_", "notarealtoken")),
            "{rendered}"
        );
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
            .with_log("/var/log/agents")
            .with_journal_limit(4096)
            .with_variable("DISABLE_AUTOUPDATER", "1")
            .inherit_environment();

        assert_eq!(settings.arguments, ["--json-schema", "{}", "--verbose"]);
        assert_eq!(settings.permissions, ["Read", "Edit"]);
        assert_eq!(settings.output_limit, 1024);
        assert_eq!(settings.line_limit, 256);
        assert_eq!(settings.schema.as_deref(), Some("{}"));
        assert_eq!(settings.instructions.as_deref(), Some("Be terse."));
        assert_eq!(settings.working_directory, Some(PathBuf::from("/w")));
        assert_eq!(settings.log, Some(PathBuf::from("/var/log/agents")));
        assert!(settings.inherit_environment);
        assert_eq!(settings.journal_limit, 4096);
        assert_eq!(
            settings
                .variables
                .get("DISABLE_AUTOUPDATER")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn allowed_names_accumulate() {
        let settings = CliSettings::default()
            .allow(["LINEAR_API_URL"])
            .allow(vec!["SSL_CERT_DIR".to_string()]);

        assert_eq!(
            settings
                .allowed
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["LINEAR_API_URL", "SSL_CERT_DIR"]
        );
    }

    #[test]
    fn the_proxy_variables_are_allowed_in_both_cases() {
        let settings = CliSettings::default()
            .allow(["LINEAR_API_URL"])
            .with_proxy_variables();

        assert_eq!(
            settings
                .allowed
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "ALL_PROXY",
                "HTTPS_PROXY",
                "HTTP_PROXY",
                "LINEAR_API_URL",
                "NO_PROXY",
                "all_proxy",
                "http_proxy",
                "https_proxy",
                "no_proxy",
            ]
        );
    }

    #[test]
    fn a_tripwire_is_kept_as_given() {
        let settings = CliSettings::default().with_tripwire(|line| line.contains("429"));
        let tripwire = settings.tripwire.expect("a tripwire");

        assert!(tripwire.trips("HTTP 429 Too Many Requests"));
        assert!(!tripwire.trips("compiling"));
    }

    #[test]
    fn a_read_only_run_allows_exactly_the_read_only_tools() {
        let settings = CliSettings::default()
            .with_permissions(["Bash", "Edit"])
            .read_only();

        assert!(settings.read_only);
        assert!(!settings.web);
        assert_eq!(settings.permissions, READ_ONLY_TOOLS);
        assert_eq!(READ_ONLY_TOOLS, ["Read", "Grep", "Glob"]);
    }

    #[test]
    fn the_web_is_an_opt_in_that_survives_a_read_only_run_in_either_order() {
        assert_eq!(WEB_TOOLS, ["WebFetch", "WebSearch"]);
        for settings in [
            CliSettings::default().allow_web().read_only(),
            CliSettings::default().read_only().allow_web(),
        ] {
            assert!(settings.web);
            assert!(settings.read_only);
            assert_eq!(settings.permissions, READ_ONLY_TOOLS);
        }
    }
}
