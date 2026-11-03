//! How a spawned coding agent is run.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use abnegate_llm::Credential;
use abnegate_llm::ProviderError;
use abnegate_secret::SecretValue;

use crate::codex_sandbox::CodexSandbox;
use crate::kind::AgentKind;
use crate::mcp::McpConfig;
use crate::mcp::McpServer;
use crate::session::Session;
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

/// Caller [arguments](CliSettings::arguments) with which Claude picks,
/// carries on or forks a session, which a run given its own
/// [session](CliSettings::session) refuses.
const CLAUDE_SESSION_FLAGS: &[&str] = &[
    "--resume",
    "-r",
    "--session-id",
    "--continue",
    "-c",
    "--fork-session",
];

/// Caller [arguments](CliSettings::arguments) with which `codex exec` picks
/// or forks a session, which a run given its own
/// [session](CliSettings::session) refuses.
const CODEX_SESSION_FLAGS: &[&str] = &["resume", "fork", "--last"];

/// Caller [arguments](CliSettings::arguments) that choose or lift Codex's
/// sandbox, directly or through a profile, which a run given its own
/// [sandbox](CliSettings::sandbox) refuses.
const CODEX_SANDBOX_FLAGS: &[&str] = &[
    "--sandbox",
    "-s",
    "--full-auto",
    "--approve-for-me",
    "--dangerously-bypass-approvals-and-sandbox",
    "--profile",
    "-p",
];

/// Codex's flags overriding one configuration value, given as `key=value`.
const CODEX_CONFIGURATION_FLAGS: &[&str] = &["-c", "--config"];

/// The configuration key naming Codex's sandbox, at the top level or in a
/// profile.
const SANDBOX_MODE_KEY: &str = "sandbox_mode";

/// The configuration key choosing the profile Codex reads its settings from.
const PROFILE_KEY: &str = "profile";

const KEY_SEPARATOR: char = '.';
const KEY_QUOTES: [char; 2] = ['"', '\''];
const LONG_FLAG: &str = "--";
const FLAG: char = '-';
const INLINE_VALUE: char = '=';

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

/// Variables that sign an agent in to a cloud provider's hosting of its
/// model, or point it at one: Amazon Bedrock and the AWS credentials it
/// reads, Google Vertex AI and the Google credentials it reads, and
/// Microsoft Foundry with the Azure identity it signs in through.
/// [`CliSettings::withhold_host_credentials`] withholds
/// them with every agent's own sign-in.
pub const CLOUD_CREDENTIALS: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "AZURE_CLIENT_ID",
    "AZURE_TENANT_ID",
    "AZURE_CLIENT_SECRET",
    "AZURE_CLIENT_CERTIFICATE_PATH",
    "AZURE_CLIENT_CERTIFICATE_PASSWORD",
    "AZURE_CLIENT_SEND_CERTIFICATE_CHAIN",
    "AZURE_FEDERATED_TOKEN_FILE",
    "AZURE_AUTHORITY_HOST",
    "AZURE_USERNAME",
    "AZURE_PASSWORD",
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
    /// The key the agent signs in with, or [`Credential::Inherited`] to use
    /// the host's existing session, which
    /// [`withhold_host_credentials`](CliSettings::withhold_host_credentials)
    /// narrows to the one the agent finds under its `HOME`.
    pub credential: Credential,
    /// How long one run may take in all, after which the agent's process
    /// group is stopped and the run fails as timed out.
    pub timeout: Duration,
    /// Bytes of the agent's prose, and of its diagnostics, kept. Prose past
    /// the limit abandons the run as malformed, with a failure that starts
    /// with [`PROSE_EXCEEDED`](crate::PROSE_EXCEEDED). Diagnostics are read
    /// to their end, every line of them checked against the
    /// [tripwire](CliSettings::tripwire), and only their last `output_limit`
    /// bytes kept. The stream around the prose is read to its end whatever
    /// its size, since none of it is kept.
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
    ///
    /// A value the child is handed under its own name, here, in
    /// [`variables`](CliSettings::variables), through
    /// [`allowed`](CliSettings::allowed) or by
    /// [`inherit_environment`](CliSettings::inherit_environment), is visible
    /// to the agent's own tools and to every stdio MCP server the agent
    /// starts: Claude Code starts them all with its own environment, less
    /// the sign-in variables it withholds. It is never sent to a remote MCP
    /// server, whose references resolve only to the
    /// [secrets](crate::McpServer::secrets) bound to it, or their defaults.
    pub environment: BTreeMap<String, SecretValue>,
    /// Set in the child's environment like `environment`, which wins over
    /// them, but never scrubbed: flags such as `DISABLE_AUTOUPDATER=1`,
    /// whose values would otherwise be redacted wherever they appear. Who
    /// can read them is as for [`environment`](CliSettings::environment).
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
    /// this way: a run that must see it sets it explicitly. Nor is a
    /// variable the agent [signs in with](crate::AgentKind::credentials),
    /// unless its [credential](CliSettings::credential) is inherited and the
    /// host's are not [withheld](CliSettings::withhold_host_credentials):
    /// beside a credential of the caller's own, the host's would be one more
    /// the agent could sign in with. A gateway such as `ANTHROPIC_BASE_URL` for
    /// a run given its own key is set explicitly.
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
    /// [`mcp`](crate::mcp). Anything else it needs, such as a proxy, is
    /// [allowed](CliSettings::allow) or set explicitly.
    pub inherit_environment: bool,
    /// Give the child none of the host's own sign-in, even when its
    /// [credential](CliSettings::credential) is inherited. These never pass
    /// from the host, whether [allowed](CliSettings::allowed) by name or
    /// [inherited](CliSettings::inherit_environment):
    ///
    /// - every variable any [agent](crate::AgentKind::ALL)
    ///   [signs in with](crate::AgentKind::credentials);
    /// - Claude Code's OAuth refresh token, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`;
    /// - the cloud sign-ins in [`CLOUD_CREDENTIALS`].
    ///
    /// Nor does any agent's [configuration](crate::AgentKind::configuration)
    /// variable, which points it at the host's stored sign-in, unless it is
    /// allowed by name. A stdio MCP server's reference to any variable in the
    /// list reads as set but empty, unless a secret is bound to the server
    /// under that name. Any other variable the host's sign-in lives in, and
    /// anything the agent finds in files under its `HOME`, such as
    /// `~/.aws/credentials`, is not withheld.
    ///
    /// Off by default. For a host whose own keys must never stand in for the
    /// sign-in it hands each run, such as a server running agents for several
    /// organisations: an inherited credential is then the one the agent finds
    /// under its `HOME`, and any other reaches it only through the
    /// credential, [`variables`](CliSettings::variables) or
    /// [`environment`](CliSettings::environment).
    pub withhold_host_credentials: bool,
    /// Extra flags passed through verbatim, after the streaming flags and
    /// before the model. Nothing here is checked against the agent, except
    /// in a [read-only](CliSettings::read_only) run, which refuses anything
    /// not in [`READ_ONLY_SWITCHES`] or [`READ_ONLY_OPTIONS`].
    pub arguments: Vec<String>,
    /// The agent's own session the run starts or resumes. `None` lets the
    /// agent start one of its own choosing. A run given one refuses a caller
    /// [argument](CliSettings::arguments) with which the agent would pick,
    /// carry on or fork a session of its own, such as Claude's `--continue`
    /// or Codex's `resume`, and a session whose id is empty or begins with
    /// `-`, which the agent would read as a flag.
    pub session: Option<Session>,
    /// The sandbox Codex runs its own tools in. `None` leaves it to Codex's
    /// configuration. A run given one refuses a caller
    /// [argument](CliSettings::arguments) that would choose or lift the
    /// sandbox instead: `--sandbox`, a profile, a bypass of approvals and
    /// sandbox, or a `-c` override of `sandbox_mode` or `profile`. Codex
    /// only.
    pub sandbox: Option<CodexSandbox>,
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
    ///
    /// Once any server here is enabled, the run loads the servers that
    /// attach from it and no MCP server of the CLI's own, the user's and the
    /// repository's included, even when none attaches.
    pub mcp: McpConfig,
    /// Where each run keeps its [execution logs](crate::log). `None` keeps none.
    pub log: Option<PathBuf>,
    /// Bytes of a run's journal the lines it printed may fill, past which
    /// they are no longer recorded.
    pub journal_limit: u64,
    /// A stderr line this returns true for settles the run as failed, in the
    /// line's own words, and the agent is stopped: an agent retrying against
    /// a rate limit is stopped instead of waited on until the timeout. Stdout
    /// is never checked, since the agent's prose can quote anything. A turn
    /// that finished anyway, its result read, still answers: the tripwire
    /// ends a wait, it never discards an answer the agent gave.
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
            withhold_host_credentials: false,
            arguments: Vec::new(),
            session: None,
            sandbox: None,
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
    /// Run `executable` instead of the agent's own command name.
    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = Some(executable.into());
        self
    }

    /// Run the agent in `directory`.
    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    /// Sign the agent in with `credential`.
    pub fn with_credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    /// Allow each run `timeout` in all. See [`CliSettings::timeout`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Keep `limit` bytes of prose and of diagnostics. See
    /// [`CliSettings::output_limit`].
    pub fn with_output_limit(mut self, limit: usize) -> Self {
        self.output_limit = limit;
        self
    }

    /// Allow one event `limit` bytes. See [`CliSettings::line_limit`].
    pub fn with_line_limit(mut self, limit: usize) -> Self {
        self.line_limit = limit;
        self
    }

    /// Set `variable` to `value` in the child's environment, as a secret
    /// the run scrubs from whatever it writes down. See
    /// [`CliSettings::environment`].
    ///
    /// The agent's own tools and every stdio MCP server it starts can read
    /// it, and a stdio server's `${VAR}` reference to it resolves to it. A
    /// remote MCP server is never sent it: a token for one is bound to that
    /// server as one of its [secrets](crate::McpServer::secrets) instead,
    /// which keeps it from every other remote server, though not from the
    /// agent's own tools or its stdio servers.
    pub fn with_environment(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.environment.insert(variable.into(), value.into());
        self
    }

    /// Set `variable` to `value` in the child's environment, never
    /// scrubbed. See [`CliSettings::variables`].
    ///
    /// The agent's own tools and every stdio MCP server it starts can read
    /// it, and a remote MCP server is never sent it, as with
    /// [`CliSettings::with_environment`].
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
    ///
    /// The agent's own tools and every stdio MCP server it starts can read
    /// an allowed value, and a remote MCP server is never sent it, as with
    /// [`CliSettings::with_environment`]: allowing `GITHUB_TOKEN` for the
    /// agent's `gh` sends it to no remote server that names it.
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

    /// Opt in to [`CliSettings::withhold_host_credentials`].
    pub fn withhold_host_credentials(mut self) -> Self {
        self.withhold_host_credentials = true;
        self
    }

    /// Append `arguments` to the flags passed through verbatim. See
    /// [`CliSettings::arguments`].
    pub fn with_arguments<I>(mut self, arguments: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Start or resume `session`. See [`CliSettings::session`].
    pub fn with_session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Run Codex's own tools in `sandbox`. See [`CliSettings::sandbox`].
    pub fn with_sandbox(mut self, sandbox: CodexSandbox) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Hold the final answer to the JSON schema `schema`, such as
    /// [`StructuredResult::SCHEMA`](crate::StructuredResult::SCHEMA).
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Append `instructions` to the agent's system prompt.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Add `permissions` to the tools the agent may use without asking. See
    /// [`CliSettings::read_only`] for what a read-only run accepts.
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

    /// Attach `server` as `name`, replacing any server attached under that
    /// name. See [`CliSettings::mcp`].
    pub fn with_mcp_server(mut self, name: impl Into<String>, server: McpServer) -> Self {
        self.mcp = self.mcp.with_server(name, server);
        self
    }

    /// Keep each run's execution logs under `root`.
    pub fn with_log(mut self, root: impl Into<PathBuf>) -> Self {
        self.log = Some(root.into());
        self
    }

    /// Let the lines a run printed fill `limit` bytes of its journal.
    pub fn with_journal_limit(mut self, limit: u64) -> Self {
        self.journal_limit = limit;
        self
    }

    /// Settle a run as failed on the first stderr line `tripwire` returns
    /// true for. See [`CliSettings::tripwire`].
    pub fn with_tripwire(
        mut self,
        tripwire: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.tripwire = Some(Tripwire::new(tripwire));
        self
    }

    /// Refuse a [session](CliSettings::session) the agent would misread, and
    /// a caller [argument](CliSettings::arguments) with which `agent` would
    /// choose what the session or the [sandbox](CliSettings::sandbox)
    /// already chose, naming the flag but never its value.
    pub(crate) fn refuse_conflicts(&self, agent: AgentKind) -> Result<(), ProviderError> {
        if let Some(session) = &self.session
            && (session.id.is_empty() || session.id.starts_with(FLAG))
        {
            return Err(ProviderError::config(
                "a session id cannot be empty or begin with '-'",
            ));
        }
        if agent == AgentKind::Claude
            && let Some(session) = &self.session
            && !session.resume
            && !is_uuid(&session.id)
        {
            return Err(ProviderError::config(
                "claude pins a session only by a UUID; resume one named otherwise",
            ));
        }
        let (session_flags, sandbox_flags, configuration_flags): (&[&str], &[&str], &[&str]) =
            match agent {
                AgentKind::Claude => (CLAUDE_SESSION_FLAGS, &[], &[]),
                AgentKind::Codex => (
                    CODEX_SESSION_FLAGS,
                    CODEX_SANDBOX_FLAGS,
                    CODEX_CONFIGURATION_FLAGS,
                ),
            };
        let chosen = [
            (self.session.is_some(), session_flags, "session"),
            (self.sandbox.is_some(), sandbox_flags, "sandbox"),
        ];
        for (index, argument) in self.arguments.iter().enumerate() {
            for (_, flags, setting) in chosen.iter().filter(|(given, ..)| *given) {
                if let Some(flag) = flags.iter().find(|flag| names(argument, flag)) {
                    return Err(ProviderError::config(format!(
                        "a run given its own {setting} cannot pass {flag}"
                    )));
                }
            }
            if self.sandbox.is_none() {
                continue;
            }
            let next = self.arguments.get(index + 1).map(String::as_str);
            for flag in configuration_flags {
                if let Some(key) = value(argument, flag, next).and_then(sandbox_key) {
                    return Err(ProviderError::config(format!(
                        "a run given its own sandbox cannot pass {flag} {key}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Whether `argument` is `flag`, with or without its value attached. A flag
/// that is not one, such as a subcommand, is only ever the whole argument.
fn names(argument: &str, flag: &str) -> bool {
    if flag.starts_with(LONG_FLAG) {
        argument
            .strip_prefix(flag)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(INLINE_VALUE))
    } else if flag.starts_with(FLAG) {
        !argument.starts_with(LONG_FLAG) && argument.starts_with(flag)
    } else {
        argument == flag
    }
}

/// The value `argument` gives `flag`: attached to it, or `next` when the
/// argument is the flag alone.
fn value<'a>(argument: &'a str, flag: &str, next: Option<&'a str>) -> Option<&'a str> {
    if !names(argument, flag) {
        return None;
    }
    match &argument[flag.len()..] {
        "" => next,
        attached => Some(attached.strip_prefix(INLINE_VALUE).unwrap_or(attached)),
    }
}

/// The key of a `key=value` configuration override when it chooses the
/// sandbox: [`SANDBOX_MODE_KEY`] at the top level or in a profile, or the
/// [`PROFILE_KEY`] whose settings Codex reads.
fn sandbox_key(setting: &str) -> Option<&str> {
    let key = setting
        .split_once(INLINE_VALUE)
        .map_or(setting, |(key, _)| key)
        .trim();
    let last = key
        .rsplit(KEY_SEPARATOR)
        .next()
        .unwrap_or(key)
        .trim()
        .trim_matches(KEY_QUOTES);
    let unquoted = key.trim_matches(KEY_QUOTES);
    (last == SANDBOX_MODE_KEY || unquoted == PROFILE_KEY).then_some(key)
}

/// Whether `id` is a UUID in its hyphenated form, as `--session-id` takes it.
fn is_uuid(id: &str) -> bool {
    const HYPHENS: [usize; 4] = [8, 13, 18, 23];
    id.len() == 36
        && id.char_indices().all(|(at, character)| {
            if HYPHENS.contains(&at) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use abnegate_llm::Credential;
    use abnegate_llm::ProviderError;
    use abnegate_secret::SecretValue;

    use super::CliSettings;
    use super::DEFAULT_JOURNAL_LIMIT;
    use super::DEFAULT_LINE_LIMIT;
    use super::DEFAULT_OUTPUT_LIMIT;
    use super::DEFAULT_TIMEOUT;
    use super::READ_ONLY_TOOLS;
    use super::WEB_TOOLS;
    use crate::codex_sandbox::CodexSandbox;
    use crate::kind::AgentKind;
    use crate::mcp::McpServer;
    use crate::session::Session;

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
        assert!(!settings.withhold_host_credentials);
        assert!(settings.arguments.is_empty());
        assert!(settings.session.is_none());
        assert!(settings.sandbox.is_none());
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
            .inherit_environment()
            .withhold_host_credentials();

        assert_eq!(settings.arguments, ["--json-schema", "{}", "--verbose"]);
        assert_eq!(settings.permissions, ["Read", "Edit"]);
        assert_eq!(settings.output_limit, 1024);
        assert_eq!(settings.line_limit, 256);
        assert_eq!(settings.schema.as_deref(), Some("{}"));
        assert_eq!(settings.instructions.as_deref(), Some("Be terse."));
        assert_eq!(settings.working_directory, Some(PathBuf::from("/w")));
        assert_eq!(settings.log, Some(PathBuf::from("/var/log/agents")));
        assert!(settings.inherit_environment);
        assert!(settings.withhold_host_credentials);
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

    #[test]
    fn a_turn_runs_in_a_session_of_the_agents_choosing_until_given_one() {
        assert_eq!(CliSettings::default().session, None);

        let session = Session::resumed("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42");
        let settings = CliSettings::default().with_session(session.clone());
        assert_eq!(settings.session, Some(session));
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            assert!(settings.refuse_conflicts(agent).is_ok(), "{agent}");
        }
    }

    #[test]
    fn codex_keeps_the_sandbox_of_its_configuration_until_given_one() {
        assert_eq!(CliSettings::default().sandbox, None);

        let settings = CliSettings::default().with_sandbox(CodexSandbox::WorkspaceWrite);
        assert_eq!(settings.sandbox, Some(CodexSandbox::WorkspaceWrite));
        assert!(settings.refuse_conflicts(AgentKind::Codex).is_ok());
    }

    /// Refuse `arguments` once `given` is set on settings carrying them, and
    /// only then, naming `refused` in the error.
    fn refused_once_given(
        agent: AgentKind,
        arguments: &[&str],
        given: impl Fn(CliSettings) -> CliSettings,
        refused: &str,
    ) {
        let settings = CliSettings::default().with_arguments(arguments.iter().copied());
        assert!(
            settings.refuse_conflicts(agent).is_ok(),
            "{agent} {arguments:?}"
        );

        let error = given(settings)
            .refuse_conflicts(agent)
            .expect_err("a refusal");
        assert!(
            matches!(&error, ProviderError::Config { detail, .. } if detail.ends_with(refused)),
            "{agent} {arguments:?}: {error:?}"
        );
    }

    #[test]
    fn a_claude_run_given_a_session_refuses_an_argument_choosing_another() {
        for (arguments, flag) in [
            (&["--resume", "6f1"][..], "--resume"),
            (&["--resume=6f1"], "--resume"),
            (&["--resume"], "--resume"),
            (&["-r", "6f1"], "-r"),
            (&["-r6f1"], "-r"),
            (&["--session-id", "6f1"], "--session-id"),
            (&["--session-id=6f1"], "--session-id"),
            (&["--continue"], "--continue"),
            (&["-c"], "-c"),
            (&["--fork-session"], "--fork-session"),
            (&["--fork-session=true"], "--fork-session"),
        ] {
            for session in [
                Session::pinned("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42"),
                Session::resumed("5b0c"),
            ] {
                refused_once_given(
                    AgentKind::Claude,
                    arguments,
                    |settings| settings.with_session(session.clone()),
                    &format!("session cannot pass {flag}"),
                );
            }
        }

        let settings = CliSettings::default()
            .with_session(Session::resumed("5b0c"))
            .with_arguments(["--effort", "high", "--resume-picker-is-not-a-flag"]);
        assert!(settings.refuse_conflicts(AgentKind::Claude).is_ok());
    }

    #[test]
    fn a_codex_run_given_a_session_refuses_an_argument_choosing_another() {
        for (arguments, flag) in [
            (&["resume", "019b"][..], "resume"),
            (&["fork", "019b"], "fork"),
            (&["--last"], "--last"),
        ] {
            for session in [Session::pinned("019b"), Session::resumed("019b")] {
                refused_once_given(
                    AgentKind::Codex,
                    arguments,
                    |settings| settings.with_session(session.clone()),
                    &format!("session cannot pass {flag}"),
                );
            }
        }

        let settings = CliSettings::default()
            .with_session(Session::resumed("019b"))
            .with_arguments(["-c", r#"model="o3""#, "--ephemeral", "--resumed-later"]);
        assert!(
            settings.refuse_conflicts(AgentKind::Codex).is_ok(),
            "codex's -c sets configuration, not a session"
        );
    }

    #[test]
    fn a_codex_run_given_a_sandbox_refuses_an_argument_choosing_another() {
        for (arguments, flag) in [
            (&["--sandbox", "danger-full-access"][..], "--sandbox"),
            (&["--sandbox=danger-full-access"], "--sandbox"),
            (&["-s", "danger-full-access"], "-s"),
            (&["-sdanger-full-access"], "-s"),
            (&["--full-auto"], "--full-auto"),
            (&["--approve-for-me"], "--approve-for-me"),
            (
                &["--dangerously-bypass-approvals-and-sandbox"],
                "--dangerously-bypass-approvals-and-sandbox",
            ),
            (&["--profile", "unconfined"], "--profile"),
            (&["-p", "unconfined"], "-p"),
        ] {
            refused_once_given(
                AgentKind::Codex,
                arguments,
                |settings| settings.with_sandbox(CodexSandbox::ReadOnly),
                &format!("sandbox cannot pass {flag}"),
            );
        }
    }

    #[test]
    fn a_codex_run_given_a_sandbox_refuses_a_configuration_override_choosing_another() {
        for (arguments, refused) in [
            (
                &["-c", r#"sandbox_mode="danger-full-access""#][..],
                "-c sandbox_mode",
            ),
            (
                &[r#"-csandbox_mode="danger-full-access""#],
                "-c sandbox_mode",
            ),
            (
                &[r#"-c=sandbox_mode="danger-full-access""#],
                "-c sandbox_mode",
            ),
            (
                &["--config", r#"sandbox_mode="danger-full-access""#],
                "--config sandbox_mode",
            ),
            (
                &[r#"--config=sandbox_mode="danger-full-access""#],
                "--config sandbox_mode",
            ),
            (
                &["-c", r#" sandbox_mode = "danger-full-access""#],
                "-c sandbox_mode",
            ),
            (
                &["-c", r#"profiles.open.sandbox_mode="danger-full-access""#],
                "-c profiles.open.sandbox_mode",
            ),
            (&["-c", r#"profile="open""#], "-c profile"),
        ] {
            refused_once_given(
                AgentKind::Codex,
                arguments,
                |settings| settings.with_sandbox(CodexSandbox::WorkspaceWrite),
                refused,
            );
        }

        let settings = CliSettings::default()
            .with_sandbox(CodexSandbox::WorkspaceWrite)
            .with_arguments([
                "-c",
                r#"model="o3""#,
                "--config",
                "sandbox_workspace_write.network_access=true",
                "-c",
            ]);
        assert!(settings.refuse_conflicts(AgentKind::Codex).is_ok());
    }

    #[test]
    fn a_pinned_claude_session_must_be_a_uuid() {
        for id in [
            "tenant-123",
            "5b0c1f7e8d434a779a3e2f0d6c1b9e42",
            "5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e4z",
        ] {
            let error = CliSettings::default()
                .with_session(Session::pinned(id))
                .refuse_conflicts(AgentKind::Claude)
                .expect_err("claude pins only a uuid");
            assert!(
                matches!(error, ProviderError::Config { .. }),
                "{id:?}: {error:?}"
            );
        }
        for (agent, session) in [
            (
                AgentKind::Claude,
                Session::pinned("5B0C1F7E-8D43-4A77-9A3E-2F0D6C1B9E42"),
            ),
            (AgentKind::Claude, Session::resumed("tenant-123")),
            (AgentKind::Codex, Session::pinned("tenant-123")),
        ] {
            assert!(
                CliSettings::default()
                    .with_session(session.clone())
                    .refuse_conflicts(agent)
                    .is_ok(),
                "{agent} {session:?}"
            );
        }
    }

    #[test]
    fn a_session_the_agent_would_read_as_a_flag_is_refused() {
        for id in ["", "-", "--dangerously-skip-permissions"] {
            for session in [Session::pinned(id), Session::resumed(id)] {
                for agent in [AgentKind::Claude, AgentKind::Codex] {
                    let error = CliSettings::default()
                        .with_session(session.clone())
                        .refuse_conflicts(agent)
                        .expect_err("a refusal");
                    assert!(
                        matches!(error, ProviderError::Config { .. }),
                        "{agent} {id:?}: {error:?}"
                    );
                }
            }
        }
    }
}
