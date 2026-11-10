use std::collections::BTreeMap;
use std::iter;
use std::path::PathBuf;
use std::time::Duration;

use abnegate_exec::EnvironmentPolicy;
use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::mcp::endpoint::McpEndpoint;
use crate::mcp::entry::Entry;
use crate::mcp::mismatch::Mismatch;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::expand;
use crate::mcp::placeholders::refers;
use crate::mcp::placeholders::resolvable;
use crate::mcp::placeholders::whole_reference;
use crate::mcp::refusal::Refusal;
use crate::mcp::segment::CLOSING;
use crate::mcp::segment::DEFAULT;
use crate::mcp::segment::OPENING;
use crate::mcp::segment::Segment;
use crate::mcp::transport::McpTransport;

const PREFIX: &str = "mcp__";
const SEPARATOR: &str = "__";
const UNDERSCORE: char = '_';
const NAME_PUNCTUATION: [char; 2] = [UNDERSCORE, '-'];

/// One MCP server, as an MCP configuration document describes it.
///
/// A server is either started as a local `command`, speaking over its stdin
/// and stdout, or reached at a `url`: exactly one of the two must be set
/// (see [`McpServer::valid`]). The same value reaches a model two ways: a
/// CLI run by a [`CliProvider`](crate::CliProvider) is given it in a file the
/// provider writes, or a launcher of its own, such as the MCP hub in
/// `abnegate-agent`, starts a command server itself and gives it
/// [`McpServer::environment_policy`].
///
/// Environment and header values are held as secrets, so a literal key never
/// reaches a log line through `Debug`, and never reaches the file a CLI is
/// given either: see [`mcp`](crate::mcp).
///
/// A `${VAR}` reference in a stdio server's command, arguments or
/// environment is resolved here, as the CLI would resolve it, against the
/// server's own [`secrets`](McpServer::secrets) first, then what the child is
/// given and then this process's environment, and the child is given the
/// resolved value under a generated name. Resolving it hands the
/// child nothing under the variable's own name, which the child holds only
/// when the caller hands it over; the CLI starts every stdio server with
/// what the child holds, so each can read the others' generated variables
/// too. A reference in a remote server's [`url`](McpServer::url) or
/// [`headers`](McpServer::headers) is resolved against that server's own
/// [`secrets`](McpServer::secrets) alone, so a remote server is sent no
/// variable the child is handed by name.
///
/// Reads and writes the `mcpServers` entry shape: `args`, `env` and `cwd` on
/// the wire, each also read under its full name here, and never the
/// [`secrets`](McpServer::secrets). The options Claude Code's entry has no
/// key for are written only when set: `tool_timeout_sec` and
/// `startup_timeout_sec` as a number of seconds, each also read as
/// `tool_timeout_seconds` and `startup_timeout_seconds`, and `required`,
/// `eager`, `exclusive` and `supervised` as `true`. A value of the wrong type
/// is reported by the field holding it and never quoted, since it may still
/// be a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct McpServer {
    /// The command that starts a stdio server, such as `uvx` or `npx`.
    pub command: Option<String>,
    /// What follows the command.
    #[serde(rename = "args")]
    pub arguments: Vec<String>,
    /// Variables a stdio server is given.
    #[serde(rename = "env")]
    pub environment: BTreeMap<String, SecretValue>,
    /// Where an HTTP or SSE server listens.
    ///
    /// Each `${VAR}` or `${VAR:-default}` reference in it is resolved when a
    /// run, or the MCP hub in `abnegate-agent`, attaches the server, as
    /// [`McpServer::endpoint`] resolves it, against the server's own
    /// [`secrets`](McpServer::secrets) and never against an environment:
    /// a variable handed to the child under its own name, as
    /// [`CliSettings::with_environment`](crate::CliSettings::with_environment),
    /// [`CliSettings::allow`](crate::CliSettings::allow) and
    /// [`CliSettings::inherit_environment`](crate::CliSettings::inherit_environment)
    /// hand one over, is never sent to a remote server. A reference to a
    /// variable with no secret takes its default. A URL that refers to
    /// anything reaches the file the CLI is given only through a generated
    /// variable holding it resolved, so neither a secret nor a default
    /// reaches the file; one that refers to nothing is written as it is, so a
    /// secret belongs in a reference. A server whose URL refers to a variable
    /// with no secret and no default, or still holds `${` once resolved,
    /// never attaches: see
    /// [`McpConfig::attachable`](crate::mcp::McpConfig::attachable).
    pub url: Option<String>,
    /// How the server is reached; implied by `command` or `url` when unset,
    /// and then left out of what this writes, since Claude Code refuses a
    /// `null` type.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpTransport>,
    /// Headers sent to an HTTP or SSE server.
    ///
    /// Each value is resolved as [`McpServer::url`] is, against the server's
    /// own [`secrets`](McpServer::secrets) alone, and reaches the file only
    /// through a generated variable holding it resolved, so no literal,
    /// default or secret reaches the file: with `TOKEN` bound,
    /// `Bearer ${TOKEN}` is written `${ABNEGATE_MCP_<token>_0}`, and the
    /// generated variable holds `Bearer ` and the token. A header's name is
    /// sent as it is, since the CLI never expands one, and a server with a
    /// name that holds `${` never attaches.
    pub headers: BTreeMap<String, SecretValue>,
    /// Values for this server's `${VAR}` references, by variable name, which
    /// no other server's references ever resolve to.
    ///
    /// A remote server's [`url`](McpServer::url) and
    /// [`headers`](McpServer::headers) resolve against them alone, a
    /// reference with no secret taking its default. A stdio server's command,
    /// arguments and [`environment`](McpServer::environment) read them
    /// first, before anything the agent is given or this process's
    /// environment holds, and a secret bound here is read as bound even under
    /// a name the agent signs in with.
    ///
    /// Each is bound in code, with [`McpServer::with_secret`] or, to a server
    /// read from a document, [`McpConfig::with_secret`](crate::mcp::McpConfig::with_secret),
    /// and never read from or written to a configuration document. A run
    /// hands the CLI a resolved value that holds one inside a generated
    /// variable of the agent's environment, never under the variable's own
    /// name, where the agent's own tools and every stdio server it starts can
    /// read it, as they can anything the agent is given: binding keeps a
    /// secret from every other remote server, not from them. The run scrubs
    /// each from what it writes down. A launcher that starts a stdio server
    /// itself reads them first too: see [`McpServer::expanded`].
    #[serde(skip)]
    pub secrets: BTreeMap<String, SecretValue>,
    /// The tools to allow without prompting, by the names the CLI gives
    /// them: the server's own name for each, with every character but
    /// letters, digits, `_` and `-` replaced by `_`. Empty allows every tool
    /// the server offers, unless the server is
    /// [exclusive](McpServer::exclusive). A launcher that starts the server
    /// itself offers a model only the tools this allows: see
    /// [`McpServer::allows`].
    pub tools: Vec<String>,
    /// The directory a stdio server starts in, or wherever its launcher
    /// chooses when unset.
    ///
    /// A rendered file never carries it, since Claude Code's MCP
    /// configuration has no such field: a Codex run, whose configuration has
    /// one, and a launcher that starts the server itself, such as the MCP
    /// hub in `abnegate-agent`, honour it.
    #[serde(rename = "cwd", skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    /// Give a stdio server its launcher's whole environment rather than
    /// [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) and
    /// `environment`: see [`McpServer::environment_policy`].
    ///
    /// Honoured only by a launcher that starts the server itself. A rendered
    /// file never carries it, since a CLI decides for itself what the servers
    /// it starts inherit.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inherit_environment: bool,
    /// Leave this server out: it is neither rendered nor launched, and none
    /// of its tools is allowed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// How long the agent waits on one call to this server's tools before
    /// giving up on it. `None` leaves it to the agent.
    ///
    /// Codex is given it as the server's `tool_timeout_sec`, rounded up to
    /// whole seconds. Claude Code has one wait for every server, so a Claude
    /// run is given the longest of its attached servers' as
    /// `MCP_TOOL_TIMEOUT` and `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`, in
    /// milliseconds rounded up and held to the 2,147,483,647 a Node timer
    /// keeps, which [`CliSettings::with_variable`](crate::CliSettings::with_variable)
    /// overrides.
    ///
    /// A configuration document holds it as `tool_timeout_sec`, a number of
    /// seconds, whole when it can be. Like every option below it, it never
    /// reaches the file Claude Code is given or a log view of the server.
    #[serde(
        rename = "tool_timeout_sec",
        skip_serializing_if = "Option::is_none",
        serialize_with = "seconds"
    )]
    pub tool_timeout: Option<Duration>,
    /// How long Codex waits for this server to start, as the server's
    /// `startup_timeout_sec`, rounded up to whole seconds. `None` leaves it
    /// to Codex. Claude Code has no such setting, so a Claude run ignores
    /// it. A configuration document holds it as `startup_timeout_sec`, a
    /// number of seconds, like [`tool_timeout`](McpServer::tool_timeout).
    #[serde(
        rename = "startup_timeout_sec",
        skip_serializing_if = "Option::is_none",
        serialize_with = "seconds"
    )]
    pub startup_timeout: Option<Duration>,
    /// Fail a Codex run that cannot start this server, rather than run it
    /// without, as the server's `required`. Claude Code has no such setting,
    /// so a Claude run ignores it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// List this server's tools to the model up front rather than behind a
    /// tool search, as Codex's `omit_tools_from = ["deferred"]`. Claude Code
    /// has no such setting, so a Claude run ignores it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub eager: bool,
    /// Hold the server to exactly [`tools`](McpServer::tools), even when the
    /// list is empty: an empty list then allows none of the server's tools
    /// rather than all of them. Codex is always given the list as the
    /// server's `enabled_tools`, so it offers the model no other tool of the
    /// server's, and a Claude run allows no tool of the server's without
    /// asking that the list does not name, though the model can still see
    /// them. The list is not a boundary Claude Code enforces, though: it
    /// still runs an unlisted tool that a settings file it loads allows, an
    /// [isolated](crate::CliSettings::isolated) run loading none but the
    /// managed settings an administrator installs, that the run's own
    /// [permissions](crate::CliSettings::permissions) allow, or that its
    /// [permission prompt](crate::CliSettings::permission_prompt) approves.
    /// A server that must refuse a tool refuses it itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exclusive: bool,
    /// Leave each call to this server's tools to Codex's own approval rules
    /// rather than approving every one in advance.
    ///
    /// By default a Codex run approves every call to an attached server's
    /// tools in advance, with the server's
    /// `default_tools_approval_mode = "approve"`, since `codex exec` has
    /// nobody to ask and whoever serves a tool can decide each call itself.
    /// A supervised server is given no approval mode, so Codex decides each
    /// call by its own default, and a call that default would stop to ask
    /// about has nobody to answer it. A Claude run is unchanged by it: what
    /// Claude Code runs without asking is what [`tools`](McpServer::tools)
    /// allows, through `--allowedTools`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub supervised: bool,
}

impl<'de> Deserialize<'de> for McpServer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entry = Value::deserialize(deserializer)?;
        Self::read(&entry).map_err(D::Error::custom)
    }
}

impl McpServer {
    /// The server a configuration document's `entry` describes, or why it
    /// does not describe one.
    pub(crate) fn read(entry: &Value) -> Result<Self, Mismatch> {
        Entry::new(entry)?.server()
    }

    /// A stdio server started as `command arguments…`.
    pub fn command(
        command: impl Into<String>,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            command: Some(command.into()),
            arguments: arguments.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// A server reached at `url`, over [`McpTransport::Http`] unless
    /// [`McpServer::with_transport`] names another.
    pub fn remote(url: impl Into<String>) -> Self {
        Self {
            url: Some(url.into()),
            ..Self::default()
        }
    }

    /// The same server, giving it `variable` set to `value`: a stdio
    /// server's [`environment`](McpServer::environment). A remote server is
    /// sent a value through a reference to one of its
    /// [`secrets`](McpServer::secrets) instead.
    pub fn with_environment(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.environment.insert(variable.into(), value.into());
        self
    }

    /// The same server, sending it the header `name` set to `value`.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<SecretValue>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    /// The same server, with `value` bound for its references to
    /// `variable`: a remote server's in its URL and headers, and a stdio
    /// server's in its command, arguments and environment. See
    /// [`McpServer::secrets`].
    ///
    /// ```
    /// use abnegate_agent_cli::McpServer;
    ///
    /// # let token = String::new();
    /// # let key = String::new();
    /// let linear = McpServer::remote("https://mcp.linear.app/mcp")
    ///     .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
    ///     .with_secret("LINEAR_TOKEN", token);
    /// let notes = McpServer::command("notes-server", ["mcp"])
    ///     .with_environment("NOTES_KEY", "${NOTES_KEY}")
    ///     .with_secret("NOTES_KEY", key);
    /// ```
    pub fn with_secret(
        mut self,
        variable: impl Into<String>,
        value: impl Into<SecretValue>,
    ) -> Self {
        self.secrets.insert(variable.into(), value.into());
        self
    }

    /// The same server, reached over `transport`.
    pub fn with_transport(mut self, transport: McpTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// The same server, allowing `tools` without prompting as well as any
    /// allowed so far.
    pub fn with_tools<I>(mut self, tools: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// The same server, started in `directory`.
    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    /// Opt in to [`McpServer::inherit_environment`].
    pub fn inherit_environment(mut self) -> Self {
        self.inherit_environment = true;
        self
    }

    /// Opt in to [`McpServer::disabled`].
    pub fn disable(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// The same server, waiting `timeout` on one call to its tools. See
    /// [`McpServer::tool_timeout`].
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = Some(timeout);
        self
    }

    /// The same server, given `timeout` to start. See
    /// [`McpServer::startup_timeout`].
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = Some(timeout);
        self
    }

    /// Opt in to [`McpServer::required`].
    pub fn require(mut self) -> Self {
        self.required = true;
        self
    }

    /// Opt in to [`McpServer::eager`].
    pub fn eager(mut self) -> Self {
        self.eager = true;
        self
    }

    /// Opt in to [`McpServer::exclusive`].
    pub fn exclusive(mut self) -> Self {
        self.exclusive = true;
        self
    }

    /// Opt in to [`McpServer::supervised`].
    pub fn supervise(mut self) -> Self {
        self.supervised = true;
        self
    }

    /// The same server with every `${VAR}` and `${VAR:-default}` in its
    /// command, arguments and environment values expanded through the
    /// server's own [`secrets`](McpServer::secrets) and then `lookup`, by the
    /// rules Claude Code expands them by, for a launcher that starts the
    /// server itself: a secret bound for a variable wins, a set variable is
    /// taken even when empty, and a reference to a variable neither gives,
    /// with no default, is left as written, as the CLI leaves it. What any
    /// other variable reads as is `lookup`'s to say: a
    /// [`CliProvider`](crate::CliProvider) reads the agent's sign-in
    /// variables as set but empty, and Claude Code its own OAuth tokens.
    ///
    /// The URL and headers are left alone, since a remote server resolves
    /// both against its [`secrets`](McpServer::secrets) alone, through
    /// [`McpServer::endpoint`], whether a CLI's run or the MCP hub in
    /// `abnegate-agent` attaches it, and so is the working directory, which
    /// neither CLI expands.
    pub fn expanded(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let lookup = |variable: &str| {
            self.secrets
                .get(variable)
                .map(|secret| secret.expose().to_string())
                .or_else(|| lookup(variable))
        };
        Self {
            command: self
                .command
                .as_deref()
                .map(|command| expand(command, &lookup)),
            arguments: self
                .arguments
                .iter()
                .map(|argument| expand(argument, &lookup))
                .collect(),
            environment: self
                .environment
                .iter()
                .map(|(variable, value)| {
                    (
                        variable.clone(),
                        SecretValue::new(expand(value.expose(), &lookup)),
                    )
                })
                .collect(),
            ..self.clone()
        }
    }

    /// The environment a launcher that starts this server itself gives it:
    /// [`EnvironmentPolicy::allowlist`], or the launcher's whole environment
    /// when [`McpServer::inherit_environment`] is set, with
    /// [`McpServer::environment`] over either.
    pub fn environment_policy(&self) -> EnvironmentPolicy {
        let base = if self.inherit_environment {
            EnvironmentPolicy::inherit()
        } else {
            EnvironmentPolicy::allowlist()
        };
        self.environment
            .iter()
            .fold(base, |policy, (variable, value)| {
                policy.with(variable, value.clone())
            })
    }

    /// Whether exactly one of `command` and `url` is set, and to something
    /// other than blank text, and any explicit transport agrees with it and
    /// is one this crate attaches a server over:
    /// [`McpTransport::Unsupported`] never is.
    pub fn valid(&self) -> bool {
        match (&self.command, &self.url, &self.transport) {
            (Some(command), None, transport) => {
                !command.trim().is_empty() && matches!(transport, None | Some(McpTransport::Stdio))
            }
            (None, Some(url), transport) => {
                !url.trim().is_empty() && transport.as_ref().is_none_or(McpTransport::remote)
            }
            _ => false,
        }
    }

    /// Whether `name`, and every tool this server names, holds only
    /// letters, digits, `_` and `-`, and so is safe in `--allowedTools`, and
    /// `name` neither holds `__`, which the CLI reads as the end of a
    /// server's name, nor ends in `_`, whose `mcp__x___tool` the CLI reads as
    /// server `x`'s tool `_tool`, so that one server's rule can never cover
    /// another's tools.
    pub fn nameable(&self, name: &str) -> bool {
        server_name(name) && self.tools.iter().all(|tool| valid_name(tool))
    }

    /// Whether this server allows `tool`, as the server itself names it:
    /// every tool when [`McpServer::tools`] is empty and the server is not
    /// [exclusive](McpServer::exclusive), and otherwise only one whose name,
    /// as the CLI gives it, the list holds: the same tools
    /// [`McpServer::allowed_tools`] allows on a CLI.
    pub fn allows(&self, tool: &str) -> bool {
        let named = cli_name(tool);
        self.allows_every_tool() || self.tools.contains(&named)
    }

    /// The `--allowedTools` entries for this server under `name`: the whole
    /// server when it names no tool and is not
    /// [exclusive](McpServer::exclusive), and otherwise its
    /// [scoped tools](McpServer::scoped_tools).
    pub fn allowed_tools(&self, name: &str) -> Vec<String> {
        if self.allows_every_tool() {
            return vec![format!("{PREFIX}{name}")];
        }
        self.scoped_tools(name)
    }

    fn allows_every_tool(&self) -> bool {
        self.tools.is_empty() && !self.exclusive
    }

    /// The `--allowedTools` entries for the tools this server names, and
    /// none for a server that names none.
    pub fn scoped_tools(&self, name: &str) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| format!("{PREFIX}{name}{SEPARATOR}{tool}"))
            .collect()
    }

    /// Whether `permission` names one tool of one MCP server, as
    /// [`McpServer::scoped_tools`] would, rather than a whole server.
    pub fn scoped(permission: &str) -> bool {
        permission
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.split_once(SEPARATOR))
            .is_some_and(|(server, tool)| valid_name(server) && valid_name(tool))
    }

    /// Why this server, configured under `name`, never attaches to a CLI's
    /// run, or none when it does.
    pub(crate) fn refusal(&self, name: &str) -> Option<Refusal> {
        self.refusal_unless_nameable(self.nameable(name))
    }

    /// Why this server, configured under `name`, never attaches to a run
    /// that takes its [`tools`](McpServer::tools) as data, as Codex takes
    /// `enabled_tools`, rather than as `--allowedTools` entries: as
    /// [`refusal`](Self::refusal), except that a tool may be named as the
    /// server itself names it, `list.files` included.
    pub(crate) fn listed_refusal(&self, name: &str) -> Option<Refusal> {
        self.refusal_unless_nameable(server_name(name))
    }

    fn refusal_unless_nameable(&self, nameable: bool) -> Option<Refusal> {
        if !self.valid() {
            return Some(Refusal::Invalid);
        }
        if !nameable {
            return Some(Refusal::Unnameable);
        }
        let url = self.url.as_deref()?;
        self.reached(url, &|_| None).err()
    }

    /// Where this server is reached, with every reference in its URL and
    /// headers resolved against its own [`secrets`](McpServer::secrets)
    /// alone, never this process's environment: as
    /// [`endpoint_with`](Self::endpoint_with) with a lookup that gives
    /// nothing.
    pub fn endpoint(&self) -> Option<McpEndpoint> {
        self.endpoint_with(&|_| None)
    }

    /// Where this server is reached, with every reference in its URL and
    /// headers resolved against its own [`secrets`](McpServer::secrets)
    /// first, then what `lookup` gives, and then its default.
    ///
    /// None for a stdio or [invalid](McpServer::valid) server, and for one
    /// whose URL or headers refer to a variable neither binds and that has
    /// no default, still hold `${` once resolved, or name a header with
    /// `${`: a server a CLI would refuse to attach for the same reason. Its
    /// name is not checked, and an `sse` server has an endpoint too.
    pub fn endpoint_with(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Option<McpEndpoint> {
        let url = self.url.as_deref().filter(|_| self.valid())?;
        self.reached(url, lookup).ok()
    }

    fn reached(
        &self,
        url: &str,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<McpEndpoint, Refusal> {
        if self.headers.keys().any(|header| header.contains(OPENING)) {
            return Err(Refusal::HeaderName);
        }
        let headers: Option<BTreeMap<String, String>> = self
            .headers
            .iter()
            .map(|(name, value)| {
                self.resolved_with(value.expose(), lookup)
                    .map(|resolved| (name.clone(), resolved))
            })
            .collect();
        let (Some(url), Some(headers)) = (self.resolved_with(url, lookup), headers) else {
            return Err(Refusal::Unbound);
        };
        if iter::once(&url)
            .chain(headers.values())
            .any(|value| value.contains(OPENING))
        {
            return Err(Refusal::Expandable);
        }
        if url.trim().is_empty() {
            return Err(Refusal::Invalid);
        }
        Ok(McpEndpoint {
            url,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name, SecretValue::new(value)))
                .collect(),
        })
    }

    /// `value`, from this server's URL or headers, with every reference
    /// resolved against the server's own [`secrets`](McpServer::secrets), one
    /// with no secret taking its default, or none when such a reference has
    /// no default.
    fn resolved(&self, value: &str) -> Option<String> {
        self.resolved_with(value, &|_| None)
    }

    /// `value` resolved as [`resolved`](Self::resolved) resolves it, with
    /// what `lookup` gives taking the place of a variable the server has no
    /// secret for.
    fn resolved_with(
        &self,
        value: &str,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        resolvable(value, &|name| {
            self.secrets.contains_key(name) || lookup(name).is_some()
        })
        .then(|| {
            expand(value, &|name| {
                self.secrets
                    .get(name)
                    .map(|secret| secret.expose().to_string())
                    .or_else(|| lookup(name))
            })
        })
    }

    /// `value` as the server is sent it: resolved, or nothing at all when it
    /// cannot be, so a server that could not attach is never sent anything
    /// for the CLI to expand.
    pub(crate) fn sent(&self, value: &str) -> String {
        self.resolved(value)
            .filter(|resolved| !resolved.contains(OPENING))
            .unwrap_or_default()
    }

    /// This server as a rendered configuration file holds it, with every
    /// environment or header value, every URL, command or argument that
    /// refers to a variable, and every remote value once resolved, replaced
    /// by a reference to a variable in `placeholders`.
    pub(crate) fn entry(&self, placeholders: &mut Placeholders) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            let command = placeholders.resolved(command, &self.secrets);
            let arguments: Vec<String> = self
                .arguments
                .iter()
                .map(|argument| placeholders.resolved(argument, &self.secrets))
                .collect();
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(arguments));
            if !self.environment.is_empty() {
                entry.insert(
                    "env".to_string(),
                    substituted(&self.environment, &self.secrets, placeholders),
                );
            }
            if let Some(transport) = &self.transport {
                entry.insert("type".to_string(), json!(transport));
            }
        } else if let Some(url) = &self.url {
            let transport = self.transport.as_ref().unwrap_or(&McpTransport::Http);
            entry.insert("type".to_string(), json!(transport));
            let url = if refers(url) {
                placeholders.hold(self.sent(url))
            } else {
                url.clone()
            };
            entry.insert("url".to_string(), json!(url));
            if !self.headers.is_empty() {
                let headers: Map<String, Value> = self
                    .headers
                    .iter()
                    .map(|(name, value)| {
                        let sent = placeholders.hold(self.sent(value.expose()));
                        (name.clone(), json!(sent))
                    })
                    .collect();
                entry.insert("headers".to_string(), Value::Object(headers));
            }
        }
        placeholders.secrets.extend(self.secrets.values().cloned());
        Value::Object(entry)
    }

    /// This server for a log line: structure intact, every environment or
    /// header value masked unless it is nothing but a `${VAR}` reference with
    /// no default, which names a secret without holding one, and in the
    /// command, the arguments and the URL every reference's default masked
    /// and anything credential shaped redacted.
    pub(crate) fn redacted(&self) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            let arguments: Vec<String> = self
                .arguments
                .iter()
                .map(|argument| logged(argument))
                .collect();
            entry.insert("command".to_string(), json!(logged(command)));
            entry.insert("args".to_string(), json!(arguments));
            if !self.environment.is_empty() {
                entry.insert("env".to_string(), masked(&self.environment));
            }
        } else if let Some(url) = &self.url {
            entry.insert("url".to_string(), json!(logged(url)));
            if !self.headers.is_empty() {
                entry.insert("headers".to_string(), masked(&self.headers));
            }
        }
        if let Some(transport) = &self.transport {
            entry.insert("type".to_string(), json!(transport));
        }
        entry.insert("tools".to_string(), json!(self.tools));
        Value::Object(entry)
    }
}

/// `name` as the CLI names a server's tool: every character but a letter, a
/// digit, `_` or `-` replaced by `_`.
fn cli_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(&character) {
                character
            } else {
                UNDERSCORE
            }
        })
        .collect()
}

/// Whether `name` can name a server: safe in `--allowedTools`, and holding
/// no `__` and no trailing `_`, as [`McpServer::nameable`] explains.
fn server_name(name: &str) -> bool {
    valid_name(name) && !name.contains(SEPARATOR) && !name.ends_with(UNDERSCORE)
}

/// The name a model is given for `server`'s tool `tool`, as a CLI spells
/// it: `mcp__<server>__<tool>`.
pub(crate) fn qualified(server: &str, tool: &str) -> String {
    format!("{PREFIX}{}{SEPARATOR}{}", cli_name(server), cli_name(tool))
}

/// The server `tool`, a name a CLI gives a server's tool as [`qualified`]
/// spells it, belongs to, or none when it names no single server's tool. A
/// server's name never holds `__`, so the first one ends it.
pub(crate) fn serving(tool: &str) -> Option<&str> {
    tool.strip_prefix(PREFIX)
        .and_then(|rest| rest.split_once(SEPARATOR))
        .filter(|(server, tool)| server_name(server) && valid_name(tool))
        .map(|(server, _)| server)
}

/// Whether `permission` is an `--allowedTools` rule for MCP tools alone: one
/// server's tool, or a whole server, as [`McpServer::allowed_tools`] writes
/// them.
pub(crate) fn mcp_rule(permission: &str) -> bool {
    McpServer::scoped(permission) || permission.strip_prefix(PREFIX).is_some_and(valid_name)
}

/// Whether `name` is safe to place in an `--allowedTools` entry, which the
/// CLI splits on commas and whitespace.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(&character)
        })
}

fn substituted(
    values: &BTreeMap<String, SecretValue>,
    secrets: &BTreeMap<String, SecretValue>,
    placeholders: &mut Placeholders,
) -> Value {
    values
        .iter()
        .map(|(key, value)| (key.clone(), json!(placeholders.substitute(value, secrets))))
        .collect::<Map<String, Value>>()
        .into()
}

/// `text` for a log line: every reference's default, literal text from the
/// configuration that may be a secret, masked, and anything credential shaped
/// redacted.
fn logged(text: &str) -> String {
    let masked: String = Segment::split(text)
        .into_iter()
        .map(|segment| match segment {
            Segment::Reference {
                name,
                default: Some(_),
                ..
            } => format!("{OPENING}{name}{DEFAULT}{REDACTED}{CLOSING}"),
            Segment::Reference { written, .. } | Segment::Literal(written) => written.to_string(),
        })
        .collect();
    redact(&masked).into_owned()
}

/// `timeout` as a configuration document holds it: a number of seconds,
/// whole when it can be.
fn seconds<S: Serializer>(timeout: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error> {
    match timeout {
        Some(timeout) if timeout.subsec_nanos() == 0 => serializer.serialize_u64(timeout.as_secs()),
        Some(timeout) => serializer.serialize_f64(timeout.as_secs_f64()),
        None => serializer.serialize_none(),
    }
}

fn masked(values: &BTreeMap<String, SecretValue>) -> Value {
    values
        .iter()
        .map(|(key, value)| {
            let value = value.expose();
            let shown = if whole_reference(value) {
                value
            } else {
                REDACTED
            };
            (key.clone(), json!(shown))
        })
        .collect::<Map<String, Value>>()
        .into()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use abnegate_exec::DEFAULT_ENVIRONMENT;
    use abnegate_secret::SecretValue;
    use serde_json::json;

    use serde_json::Value;

    use super::McpServer;
    use super::mcp_rule;
    use super::qualified;
    use super::serving;
    use crate::mcp::endpoint::McpEndpoint;
    use crate::mcp::placeholders::Placeholders;
    use crate::mcp::placeholders::expand;
    use crate::mcp::refusal::Refusal;
    use crate::mcp::transport::McpTransport;

    fn secrets(pairs: &[(&str, &str)]) -> BTreeMap<String, SecretValue> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), SecretValue::new(*value)))
            .collect()
    }

    fn stdio() -> McpServer {
        McpServer {
            command: Some("uvx".to_string()),
            arguments: vec!["mcp-server-appwrite".to_string()],
            ..McpServer::default()
        }
    }

    fn http() -> McpServer {
        McpServer {
            url: Some("https://example.com/mcp".to_string()),
            ..McpServer::default()
        }
    }

    #[test]
    fn exactly_one_transport_with_an_agreeing_type_is_valid() {
        assert!(stdio().valid());
        assert!(
            McpServer {
                transport: Some(McpTransport::Stdio),
                ..stdio()
            }
            .valid()
        );
        assert!(http().valid());
        for transport in [McpTransport::Http, McpTransport::Sse] {
            assert!(
                McpServer {
                    transport: Some(transport),
                    ..http()
                }
                .valid()
            );
        }
    }

    /// A blank command starts nothing and a blank URL reaches nothing, and a
    /// strict CLI rejects every other server along with one.
    #[test]
    fn a_blank_command_or_url_is_never_valid() {
        for server in [
            McpServer::command("", Vec::<String>::new()),
            McpServer::command("   ", ["mcp"]),
            McpServer::remote(""),
            McpServer::remote(" \t"),
        ] {
            assert!(!server.valid(), "{server:?}");
        }
    }

    #[test]
    fn a_transport_this_crate_does_not_attach_over_is_never_valid() {
        for server in [stdio(), http()] {
            assert!(
                !server
                    .with_transport(McpTransport::Unsupported("ws".to_string()))
                    .valid()
            );
        }
    }

    #[test]
    fn neither_both_or_a_contradicting_type_is_invalid() {
        assert!(!McpServer::default().valid());
        assert!(
            !McpServer {
                url: Some("https://example.com/mcp".to_string()),
                ..stdio()
            }
            .valid()
        );
        assert!(
            !McpServer {
                transport: Some(McpTransport::Http),
                ..stdio()
            }
            .valid()
        );
        assert!(
            !McpServer {
                transport: Some(McpTransport::Stdio),
                ..http()
            }
            .valid()
        );
    }

    #[test]
    fn a_server_without_a_tool_list_allows_all_of_its_tools() {
        assert_eq!(stdio().allowed_tools("appwrite"), ["mcp__appwrite"]);

        let scoped = McpServer {
            tools: vec!["list_datasources".to_string(), "query".to_string()],
            ..stdio()
        };
        assert_eq!(
            scoped.allowed_tools("grafana"),
            ["mcp__grafana__list_datasources", "mcp__grafana__query"]
        );
    }

    #[test]
    fn only_a_single_named_tool_of_a_server_counts_as_scoped() {
        for permission in ["mcp__grafana__query", "mcp__my-server__list_things"] {
            assert!(McpServer::scoped(permission), "{permission}");
        }
        for permission in [
            "mcp__grafana",
            "mcp__grafana__",
            "mcp____query",
            "mcp__grafana__query Bash",
            "mcp__grafana__query,Bash",
            "mcp__grafana__*",
            "Bash",
            "Read",
        ] {
            assert!(!McpServer::scoped(permission), "{permission}");
        }
    }

    #[test]
    fn a_server_allows_every_tool_unless_it_names_some() {
        assert!(stdio().allows("anything"));

        let scoped = stdio().with_tools(["search"]);
        assert!(scoped.allows("search"));
        assert!(!scoped.allows("delete"));
        assert_eq!(scoped.allowed_tools("docs"), ["mcp__docs__search"]);
    }

    /// The CLI names a server's tool with every character but letters,
    /// digits, `_` and `-` replaced by `_`, and `tools` names tools the way
    /// the CLI does, so a launcher of its own must compare them the same way
    /// or allow a different set of tools than the CLI.
    #[test]
    fn a_tool_is_allowed_by_the_name_the_cli_gives_it() {
        let scoped = stdio().with_tools(["get_item", "list-items"]);

        assert!(scoped.allows("get.item"));
        assert!(scoped.allows("get item"));
        assert!(scoped.allows("get_item"));
        assert!(scoped.allows("list-items"));
        assert!(!scoped.allows("get-item"));
        assert!(!scoped.allows("delete"));
    }

    #[test]
    fn a_server_without_a_tool_list_has_no_scoped_tools() {
        assert!(stdio().scoped_tools("appwrite").is_empty());
        assert_eq!(
            McpServer {
                tools: vec!["query".to_string()],
                ..stdio()
            }
            .scoped_tools("grafana"),
            ["mcp__grafana__query"]
        );
    }

    #[test]
    fn a_stdio_entry_moves_a_reference_out_to_be_resolved() {
        let server = McpServer {
            environment: secrets(&[("APPWRITE_API_KEY", "${APPWRITE_API_KEY}")]),
            ..stdio()
        };

        let mut placeholders = Placeholders::under("TEST");
        let entry = server.entry(&mut placeholders);
        assert_eq!(entry["command"], "uvx");
        assert_eq!(entry["args"][0], "mcp-server-appwrite");
        assert_eq!(entry["env"]["APPWRITE_API_KEY"], "${ABNEGATE_MCP_TEST_0}");
        assert!(entry.get("type").is_none());
        assert!(entry.get("url").is_none());
        assert!(placeholders.environment.is_empty());
        assert_eq!(
            placeholders
                .templates
                .get("ABNEGATE_MCP_TEST_0")
                .map(|template| template.value.expose()),
            Some("${APPWRITE_API_KEY}")
        );
    }

    #[test]
    fn a_json_blob_of_references_moves_out_of_the_entry_to_be_expanded() {
        let headers = "{\"CF-Access-Client-Id\": \"${CF_ACCESS_CLIENT_ID}\", \"CF-Access-Client-Secret\": \"${CF_ACCESS_CLIENT_SECRET}\"}";
        let server = McpServer {
            command: Some("uvx".to_string()),
            arguments: vec!["mcp-grafana".to_string()],
            environment: secrets(&[
                (
                    "GRAFANA_SERVICE_ACCOUNT_TOKEN",
                    "${GRAFANA_SERVICE_ACCOUNT_TOKEN}",
                ),
                ("GRAFANA_EXTRA_HEADERS", headers),
            ]),
            ..McpServer::default()
        };
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders);

        assert_eq!(
            entry["env"]["GRAFANA_EXTRA_HEADERS"],
            "${ABNEGATE_MCP_TEST_0}"
        );
        assert_eq!(
            entry["env"]["GRAFANA_SERVICE_ACCOUNT_TOKEN"],
            "${ABNEGATE_MCP_TEST_1}"
        );
        assert_eq!(
            placeholders
                .templates
                .values()
                .map(|template| template.value.expose())
                .collect::<Vec<_>>(),
            [headers, "${GRAFANA_SERVICE_ACCOUNT_TOKEN}"]
        );
        assert!(placeholders.environment.is_empty());
    }

    #[test]
    fn an_http_entry_carries_only_http_fields() {
        let server = McpServer {
            transport: Some(McpTransport::Http),
            headers: secrets(&[("Authorization", "Bearer ${TOKEN}")]),
            ..http()
        }
        .with_secret("TOKEN", "bound-token");

        let mut placeholders = Placeholders::under("TEST");
        let entry = server.entry(&mut placeholders);
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        assert_eq!(entry["headers"]["Authorization"], "${ABNEGATE_MCP_TEST_0}");
        assert!(entry.get("command").is_none());
        assert!(entry.get("args").is_none());
        assert!(entry.get("env").is_none());
    }

    /// Claude Code expands a remote server's URL and headers against the
    /// environment it was started with, which holds whatever the caller
    /// handed the agent's own tools. Each reference is resolved here instead,
    /// against the server's own secrets alone, and the file holds only a
    /// generated variable with the value resolved.
    #[test]
    fn a_remote_value_resolves_against_the_servers_own_secrets_alone() {
        let server = McpServer::remote("https://${HOST}/mcp?team=${TEAM:-core}")
            .with_header("Authorization", "Bearer ${TOKEN}")
            .with_header("X-Client", "id=${CLIENT};v=1")
            .with_secret("HOST", "mcp.example.com")
            .with_secret("TOKEN", "bound-token")
            .with_secret("CLIENT", "bound-client");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders);

        assert_eq!(entry["url"], "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(entry["headers"]["Authorization"], "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(entry["headers"]["X-Client"], "${ABNEGATE_MCP_TEST_2}");
        assert!(!entry.to_string().contains("bound-"), "{entry}");
        assert_eq!(
            placeholders
                .environment
                .iter()
                .map(|(variable, value)| (variable.as_str(), value.expose()))
                .collect::<Vec<_>>(),
            [
                (
                    "ABNEGATE_MCP_TEST_0",
                    "https://mcp.example.com/mcp?team=core"
                ),
                ("ABNEGATE_MCP_TEST_1", "Bearer bound-token"),
                ("ABNEGATE_MCP_TEST_2", "id=bound-client;v=1"),
            ]
        );
        assert!(placeholders.templates.is_empty());
        assert_eq!(
            placeholders
                .secrets
                .iter()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["bound-client", "mcp.example.com", "bound-token"]
        );
    }

    /// The child holds every generated variable, and the CLI expands one
    /// wherever the file names it, so a remote server able to name the one
    /// holding another server's literal would be sent that literal. Its
    /// reference, even to that very name, resolves against its own secrets
    /// alone, here to its default.
    #[test]
    fn a_remote_server_naming_another_servers_generated_variable_is_sent_its_own_default() {
        let grafana = stdio().with_environment("GRAFANA_TOKEN", "glsa-literal-secret");
        let collector = McpServer::remote("https://collector.example/${ABNEGATE_MCP_TEST_0:-open}")
            .with_header("X-Collected", "${ABNEGATE_MCP_TEST_0:-none}");
        let mut placeholders = Placeholders::under("TEST");

        let stdio = grafana.entry(&mut placeholders);
        let remote = collector.entry(&mut placeholders);

        assert_eq!(stdio["env"]["GRAFANA_TOKEN"], "${ABNEGATE_MCP_TEST_0}");
        let child = |name: &str| {
            placeholders
                .environment
                .get(name)
                .map(|value| value.expose().to_string())
        };
        let sent = |value: &Value| {
            let written = value.as_str().expect("text");
            expand(&expand(written, &child), &child)
        };
        assert_eq!(sent(&remote["url"]), "https://collector.example/open");
        assert_eq!(sent(&remote["headers"]["X-Collected"]), "none");
    }

    /// A default is literal text from the configuration, and may be a
    /// secret, so it moves out of the file with the rest of the value; a
    /// secret bound as empty is used as it is, as a variable set to nothing
    /// is.
    #[test]
    fn a_remote_reference_with_no_secret_takes_its_default() {
        let server = http()
            .with_header("X-Client", "id=${CF_ID:-anonymous}")
            .with_header("X-Empty", "[${EMPTY:-unused}]")
            .with_header("X-Key", "${KEY:-default-literal-key}")
            .with_secret("EMPTY", "");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders);

        let written = entry.to_string();
        assert!(!written.contains("anonymous"), "{written}");
        assert!(!written.contains("default-literal"), "{written}");
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["id=anonymous", "[]", "default-literal-key"]
        );
    }

    #[test]
    fn a_literal_secret_in_a_remote_header_reaches_the_child_only_through_a_placeholder() {
        let server = http()
            .with_header("Authorization", "Bearer sk-live-secret")
            .with_header(
                "X-Session",
                "secret=sk-live-secret;user=${USER_NAME:-agent}",
            );
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders);

        assert!(!entry.to_string().contains("sk-live-secret"), "{entry}");
        assert_eq!(entry["headers"]["Authorization"], "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(entry["headers"]["X-Session"], "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["Bearer sk-live-secret", "secret=sk-live-secret;user=agent"]
        );
    }

    #[test]
    fn a_url_without_a_type_defaults_to_http_and_sse_is_kept() {
        assert_eq!(
            http().entry(&mut Placeholders::under("TEST"))["type"],
            "http"
        );
        assert_eq!(
            McpServer {
                transport: Some(McpTransport::Sse),
                ..http()
            }
            .entry(&mut Placeholders::under("TEST"))["type"],
            "sse"
        );
    }

    #[test]
    fn a_literal_secret_never_reaches_the_entry() {
        let server = McpServer {
            environment: secrets(&[("GRAFANA_TOKEN", "glsa_realsecret")]),
            arguments: vec!["--url".to_string(), "${GRAFANA_URL}".to_string()],
            ..stdio()
        };
        let remote = McpServer {
            headers: secrets(&[("Authorization", "Bearer sk-live-secret")]),
            url: Some("https://${MCP_HOST}/mcp".to_string()),
            ..McpServer::default()
        }
        .with_secret("MCP_HOST", "mcp.example.com");
        let mut placeholders = Placeholders::under("TEST");

        let entries = [
            server.entry(&mut placeholders),
            remote.entry(&mut placeholders),
        ];

        for entry in &entries {
            let rendered = entry.to_string();
            assert!(!rendered.contains("glsa_realsecret"), "{rendered}");
            assert!(!rendered.contains("sk-live-secret"), "{rendered}");
        }
        assert_eq!(entries[0]["args"][1], "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(entries[0]["env"]["GRAFANA_TOKEN"], "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(entries[1]["url"], "${ABNEGATE_MCP_TEST_2}");
        assert_eq!(
            entries[1]["headers"]["Authorization"],
            "${ABNEGATE_MCP_TEST_3}"
        );
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            [
                "glsa_realsecret",
                "https://mcp.example.com/mcp",
                "Bearer sk-live-secret"
            ]
        );
        assert_eq!(
            placeholders
                .templates
                .values()
                .map(|template| template.value.expose())
                .collect::<Vec<_>>(),
            ["${GRAFANA_URL}"],
            "only a stdio server's reference is resolved against the child"
        );
    }

    #[test]
    fn a_log_view_masks_every_value_that_is_not_a_bare_reference() {
        let server = McpServer {
            command: Some("uvx".to_string()),
            arguments: vec!["mcp-grafana".to_string()],
            environment: secrets(&[
                ("GRAFANA_URL", "https://tel.example.com/"),
                (
                    "GRAFANA_SERVICE_ACCOUNT_TOKEN",
                    "${GRAFANA_SERVICE_ACCOUNT_TOKEN}",
                ),
                ("GRAFANA_TOKEN_LITERAL", "glsa_realsecret"),
                (
                    "GRAFANA_EXTRA_HEADERS",
                    "{\"CF-Access-Client-Id\": \"${CF_ID}\"}",
                ),
            ]),
            tools: vec!["list_datasources".to_string()],
            ..McpServer::default()
        };

        let view = server.redacted();
        let environment = &view["env"];
        assert_eq!(environment["GRAFANA_URL"], "[REDACTED]");
        assert_eq!(
            environment["GRAFANA_SERVICE_ACCOUNT_TOKEN"],
            "${GRAFANA_SERVICE_ACCOUNT_TOKEN}"
        );
        assert_eq!(environment["GRAFANA_TOKEN_LITERAL"], "[REDACTED]");
        assert_eq!(environment["GRAFANA_EXTRA_HEADERS"], "[REDACTED]");
        assert_eq!(view["command"], "uvx");
        assert_eq!(view["args"][0], "mcp-grafana");
        assert_eq!(view["tools"][0], "list_datasources");
        assert!(!view.to_string().contains("glsa_realsecret"));
    }

    /// A default is literal text from the configuration, so it may be a
    /// secret, and so may a `${...}` that names no variable.
    #[test]
    fn a_log_view_masks_a_default_and_anything_that_names_no_variable() {
        let server = stdio()
            .with_environment("DEFAULTED", "${T:-marker-default-secret}")
            .with_environment("UNNAMED", "${hunter2-password}")
            .with_environment("NAMED", "${TOKEN}");
        let remote = http().with_header("Authorization", "${T:-marker-default-secret}");

        let view = server.redacted();
        assert_eq!(view["env"]["DEFAULTED"], "[REDACTED]");
        assert_eq!(view["env"]["UNNAMED"], "[REDACTED]");
        assert_eq!(view["env"]["NAMED"], "${TOKEN}");
        assert_eq!(remote.redacted()["headers"]["Authorization"], "[REDACTED]");
    }

    /// A default in a URL, a command or an argument is literal text from the
    /// configuration too, and the file never holds it, so the log view masks
    /// it as it masks one in an environment or header value, keeping the
    /// rest of the value legible.
    #[test]
    fn a_log_view_masks_a_default_in_a_url_command_or_argument() {
        let remote = McpServer::remote(
            "https://${HOST:-internal-default-host}/mcp?team=${TEAM:-url-default-marker}&region=${REGION}",
        );
        let stdio = McpServer::command(
            "${LAUNCHER:-command-default-marker}",
            [
                "--team=${TEAM:-argument-default-marker}",
                "--region=${REGION}",
            ],
        );

        let url = remote.redacted()["url"].clone();
        let view = stdio.redacted();

        assert_eq!(
            url,
            "https://${HOST:-[REDACTED]}/mcp?team=${TEAM:-[REDACTED]}&region=${REGION}"
        );
        assert_eq!(view["command"], "${LAUNCHER:-[REDACTED]}");
        assert_eq!(
            view["args"],
            json!(["--team=${TEAM:-[REDACTED]}", "--region=${REGION}"])
        );
    }

    #[test]
    fn a_log_view_masks_headers_too() {
        let server = McpServer {
            transport: Some(McpTransport::Http),
            headers: secrets(&[
                ("Authorization", "Bearer sk-live-secret"),
                ("X-Token", "${TOKEN}"),
            ]),
            ..http()
        };

        let view = server.redacted();
        assert_eq!(view["headers"]["Authorization"], "[REDACTED]");
        assert_eq!(view["headers"]["X-Token"], "${TOKEN}");
        assert_eq!(view["url"], "https://example.com/mcp");
        assert_eq!(view["type"], "http");
    }

    #[test]
    fn a_log_view_redacts_credentials_in_arguments_and_urls() {
        let key = concat!("sk-ant-", "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let stdio = McpServer {
            arguments: vec!["--api-key".to_string(), key.to_string()],
            ..stdio()
        };
        let remote = McpServer {
            url: Some(format!("https://example.com/mcp?token={key}")),
            ..McpServer::default()
        };

        for view in [stdio.redacted(), remote.redacted()] {
            assert!(
                !view.to_string().contains(concat!("sk-ant-", "api03-AAAA")),
                "{view}"
            );
        }
        assert_eq!(stdio.redacted()["args"][0], "--api-key");
    }

    #[test]
    fn debug_never_prints_a_literal_secret() {
        let server = McpServer {
            environment: secrets(&[("TOKEN", "glsa_realsecret")]),
            headers: secrets(&[("Authorization", "Bearer sk-live-secret")]),
            ..stdio()
        };

        let debug = format!("{server:?}");
        assert!(!debug.contains("glsa_realsecret"), "{debug}");
        assert!(!debug.contains("sk-live-secret"), "{debug}");
    }

    #[test]
    fn a_server_reads_from_the_configuration_names_the_cli_uses() {
        let server: McpServer = serde_json::from_value(json!({
            "command": "npx",
            "args": ["-y", "server"],
            "env": {"KEY": "${KEY}"},
            "type": "stdio",
            "tools": ["search"]
        }))
        .expect("a server");

        assert_eq!(server.command.as_deref(), Some("npx"));
        assert_eq!(server.arguments, ["-y", "server"]);
        assert_eq!(
            server.environment.get("KEY").map(SecretValue::expose),
            Some("${KEY}")
        );
        assert_eq!(server.transport, Some(McpTransport::Stdio));
        assert_eq!(server.tools, ["search"]);
        assert!(server.valid());
    }

    #[test]
    fn a_command_server_built_in_code_names_its_command_and_arguments() {
        let server = McpServer::command("notes-server", ["mcp"]);

        assert_eq!(server.command.as_deref(), Some("notes-server"));
        assert_eq!(server.arguments, ["mcp"]);
        assert!(server.environment.is_empty());
        assert!(server.url.is_none());
        assert!(server.working_directory.is_none());
        assert!(!server.inherit_environment);
        assert!(!server.disabled);
        assert!(server.valid());
    }

    #[test]
    fn a_remote_server_built_in_code_is_reached_at_its_url() {
        let server = McpServer::remote("https://example.com/mcp")
            .with_header("Authorization", "Bearer ${TOKEN}")
            .with_secret("TOKEN", "bound-token")
            .with_tools(["search"]);

        assert_eq!(server.url.as_deref(), Some("https://example.com/mcp"));
        assert!(server.command.is_none());
        assert_eq!(server.tools, ["search"]);
        assert!(server.valid());
        assert_eq!(
            server
                .with_transport(McpTransport::Sse)
                .entry(&mut Placeholders::under("TEST"))["type"],
            "sse"
        );
    }

    #[test]
    fn every_builder_sets_the_field_it_names() {
        let server = McpServer::command("notes-server", ["mcp"])
            .with_environment("NOTES_TOKEN", "token")
            .with_secret("NOTES_SECRET", "secret")
            .with_working_directory("/srv/notes")
            .with_transport(McpTransport::Stdio)
            .inherit_environment()
            .disable();

        assert_eq!(
            server
                .environment
                .get("NOTES_TOKEN")
                .map(SecretValue::expose),
            Some("token")
        );
        assert_eq!(
            server.secrets.get("NOTES_SECRET").map(SecretValue::expose),
            Some("secret")
        );
        assert_eq!(server.working_directory, Some(PathBuf::from("/srv/notes")));
        assert_eq!(server.transport, Some(McpTransport::Stdio));
        assert!(server.inherit_environment);
        assert!(server.disabled);

        let server = McpServer::remote("http://127.0.0.1:9/mcp")
            .with_tool_timeout(Duration::from_secs(1800))
            .with_startup_timeout(Duration::from_secs(30))
            .require()
            .eager()
            .exclusive()
            .supervise();
        assert_eq!(server.tool_timeout, Some(Duration::from_secs(1800)));
        assert_eq!(server.startup_timeout, Some(Duration::from_secs(30)));
        assert!(server.required);
        assert!(server.eager);
        assert!(server.exclusive);
        assert!(server.supervised);

        let plain = McpServer::remote("http://127.0.0.1:9/mcp");
        assert_eq!(plain.tool_timeout, None);
        assert_eq!(plain.startup_timeout, None);
        assert!(!plain.required && !plain.eager && !plain.exclusive && !plain.supervised);
    }

    /// Every option survives a document's round trip, written under its own
    /// key with each wait in seconds, while the log view stays as it was.
    #[test]
    fn every_option_survives_a_round_trip_through_a_document() {
        let plain = McpServer::remote("http://127.0.0.1:9/mcp").with_tools(["a"]);
        let optioned = plain
            .clone()
            .with_tool_timeout(Duration::from_secs(1800))
            .with_startup_timeout(Duration::from_millis(29_500))
            .require()
            .eager()
            .exclusive()
            .supervise();

        let written = serde_json::to_value(&optioned).expect("serialisable");

        assert_eq!(
            written,
            json!({
                "url": "http://127.0.0.1:9/mcp",
                "args": [],
                "command": null,
                "env": {},
                "headers": {},
                "tools": ["a"],
                "tool_timeout_sec": 1800,
                "startup_timeout_sec": 29.5,
                "required": true,
                "eager": true,
                "exclusive": true,
                "supervised": true,
            })
        );
        let read: McpServer = serde_json::from_value(written).expect("a server");
        assert_eq!(read, optioned);
        assert_eq!(optioned.redacted(), plain.redacted());
    }

    /// The longest wait a [`Duration`] holds is written as a number of
    /// seconds past what one reads back exactly, and still reads back as the
    /// longest rather than failing the document.
    #[test]
    fn the_longest_wait_survives_a_round_trip_through_a_document() {
        let longest = McpServer::remote("http://127.0.0.1:9/mcp")
            .with_tool_timeout(Duration::MAX)
            .with_startup_timeout(Duration::MAX);

        let written = serde_json::to_string(&longest).expect("serialisable");
        let read: McpServer = serde_json::from_str(&written).expect("a server");

        assert_eq!(read, longest);
    }

    /// An exclusive server naming no tool allows none, and reads back as
    /// one, never as a server that allows every tool.
    #[test]
    fn an_exclusive_server_naming_no_tool_reads_back_allowing_none() {
        let written = serde_json::to_string(&http().exclusive()).expect("serialisable");

        let read: McpServer = serde_json::from_str(&written).expect("a server");

        assert!(read.exclusive);
        assert!(read.tools.is_empty());
        assert!(!read.allows("anything"));
        assert!(read.allowed_tools("docs").is_empty());
    }

    /// A wait reads as whole or fractional seconds, under its full name too,
    /// and anything but a number of seconds names its field.
    #[test]
    fn a_wait_reads_as_a_number_of_seconds() {
        let read: McpServer = serde_json::from_value(json!({
            "url": "http://127.0.0.1:9/mcp",
            "tool_timeout_seconds": 0.25,
            "startup_timeout_sec": 30,
        }))
        .expect("a server");
        assert_eq!(read.tool_timeout, Some(Duration::from_millis(250)));
        assert_eq!(read.startup_timeout, Some(Duration::from_secs(30)));

        for (entry, field) in [
            (json!({"tool_timeout_sec": "30"}), "tool_timeout_sec"),
            (json!({"tool_timeout_sec": -1}), "tool_timeout_sec"),
            (
                json!({"startup_timeout_seconds": true}),
                "startup_timeout_sec",
            ),
            (
                json!({"tool_timeout_sec": 1, "tool_timeout_seconds": 1}),
                "tool_timeout_sec",
            ),
        ] {
            let error = serde_json::from_value::<McpServer>(entry.clone())
                .expect_err("a mismatch")
                .to_string();
            assert!(error.contains(&format!("`{field}`")), "{entry}: {error}");
        }
    }

    /// An exclusive server allows exactly the tools it names, so naming none
    /// allows none, where a server that is not exclusive allows them all.
    #[test]
    fn an_exclusive_server_allows_exactly_the_tools_it_names() {
        let none = stdio().exclusive();
        assert!(!none.allows("anything"));
        assert!(none.allowed_tools("docs").is_empty());
        assert!(none.scoped_tools("docs").is_empty());

        let some = stdio().with_tools(["search"]).exclusive();
        assert!(some.allows("search"));
        assert!(!some.allows("delete"));
        assert_eq!(some.allowed_tools("docs"), ["mcp__docs__search"]);
        assert_eq!(some.scoped_tools("docs"), ["mcp__docs__search"]);
    }

    #[test]
    fn a_qualified_tool_names_the_server_serving_it() {
        assert_eq!(serving("mcp__host__approve_tool_call"), Some("host"));
        assert_eq!(serving("mcp__host-2__a__b"), Some("host-2"));
        for unserved in [
            "mcp__host",
            "mcp__host__",
            "Bash",
            "mcp____tool",
            "mcp__a b__c",
        ] {
            assert_eq!(serving(unserved), None, "{unserved}");
        }
    }

    #[test]
    fn an_mcp_rule_names_a_server_or_one_of_its_tools_and_nothing_else() {
        for rule in ["mcp__host", "mcp__host__read_file"] {
            assert!(mcp_rule(rule), "{rule}");
        }
        for rule in [
            "Bash",
            "mcp__",
            "mcp__host,Bash",
            "mcp__host Bash",
            "Read(./**)",
        ] {
            assert!(!mcp_rule(rule), "{rule}");
        }
    }

    #[test]
    fn the_child_environment_is_the_allowlist_and_the_server_environment_unless_it_inherits() {
        let server =
            McpServer::command("notes-server", ["mcp"]).with_environment("NOTES_TOKEN", "token");

        let policy = server.environment_policy();
        assert!(!policy.inherits());
        assert_eq!(
            policy.get("NOTES_TOKEN").as_ref().map(SecretValue::expose),
            Some("token")
        );
        for name in policy.names().filter(|name| *name != "NOTES_TOKEN") {
            assert!(DEFAULT_ENVIRONMENT.contains(&name), "{name}");
        }

        assert!(server.inherit_environment().environment_policy().inherits());
    }

    #[test]
    fn the_cursor_shape_reads_back_with_its_secrets_intact() {
        let server: McpServer = serde_json::from_value(json!({
            "command": "notes-server",
            "args": ["mcp"],
            "env": {"NOTES_TOKEN": "token"},
            "cwd": "/srv/notes"
        }))
        .expect("a server");

        assert_eq!(server.arguments, ["mcp"]);
        assert_eq!(
            server
                .environment
                .get("NOTES_TOKEN")
                .map(SecretValue::expose),
            Some("token")
        );
        assert_eq!(server.working_directory, Some(PathBuf::from("/srv/notes")));
        let written = serde_json::to_value(&server).expect("serialisable");
        assert_eq!(written["env"]["NOTES_TOKEN"], "token");
        assert_eq!(written["args"][0], "mcp");
        assert_eq!(written["cwd"], "/srv/notes");
    }

    #[test]
    fn every_field_also_reads_under_its_full_name() {
        let server: McpServer = serde_json::from_value(json!({
            "command": "notes-server",
            "arguments": ["mcp"],
            "environment": {"NOTES_TOKEN": "token"},
            "working_directory": "/srv/notes",
            "inherit_environment": true,
            "disabled": true
        }))
        .expect("a server");

        assert_eq!(
            server,
            McpServer::command("notes-server", ["mcp"])
                .with_environment("NOTES_TOKEN", "token")
                .with_working_directory("/srv/notes")
                .inherit_environment()
                .disable()
        );
    }

    #[test]
    fn a_server_written_before_these_fields_existed_writes_the_same_keys() {
        let written = serde_json::to_value(stdio()).expect("serialisable");

        let keys: Vec<&String> = written.as_object().expect("an object").keys().collect();
        assert_eq!(keys, ["args", "command", "env", "headers", "tools", "url"]);

        let written = serde_json::to_value(
            stdio()
                .with_working_directory("/srv")
                .inherit_environment()
                .disable(),
        )
        .expect("serialisable");
        assert_eq!(written["cwd"], "/srv");
        assert_eq!(written["inherit_environment"], true);
        assert_eq!(written["disabled"], true);
    }

    #[test]
    fn an_entry_never_carries_a_working_directory() {
        for server in [
            stdio().with_working_directory("/srv/notes"),
            http().with_working_directory("/srv/notes"),
        ] {
            let entry = server.entry(&mut Placeholders::under("TEST"));
            assert!(entry.get("cwd").is_none(), "{entry}");
        }
    }

    #[test]
    fn expanding_a_server_expands_its_command_arguments_and_environment_as_a_cli_would() {
        let lookup = |name: &str| (name == "HOST").then(|| "example.com".to_string());
        let server = McpServer::command("${LAUNCHER:-uvx}", ["--host=${HOST}", "${MISSING}"])
            .with_environment("URL", "https://${HOST}/mcp")
            .with_environment("TOKEN", "${MISSING}")
            .with_working_directory("/srv/${HOST}")
            .with_tools(["search"]);

        let expanded = server.expanded(&lookup);

        assert_eq!(expanded.command.as_deref(), Some("uvx"));
        assert_eq!(expanded.arguments, ["--host=example.com", "${MISSING}"]);
        assert_eq!(
            expanded
                .environment
                .iter()
                .map(|(variable, value)| (variable.as_str(), value.expose()))
                .collect::<Vec<_>>(),
            [("TOKEN", "${MISSING}"), ("URL", "https://example.com/mcp")]
        );
        assert_eq!(
            expanded.working_directory,
            Some(PathBuf::from("/srv/${HOST}"))
        );
        assert_eq!(expanded.tools, ["search"]);
    }

    /// A launcher that starts a stdio server itself reads the secrets bound
    /// to it first, as a CLI's run does, whatever its lookup gives.
    #[test]
    fn expanding_a_server_reads_its_own_secrets_before_the_lookup() {
        let lookup = |name: &str| Some(format!("{name}-from-the-lookup"));
        let server = McpServer::command("${LAUNCHER:-uvx}", ["--token=${T}", "--home=${HOME}"])
            .with_environment("T", "${T:-unused}")
            .with_secret("T", "bound");

        let expanded = server.expanded(&lookup);

        assert_eq!(
            expanded.command.as_deref(),
            Some("LAUNCHER-from-the-lookup")
        );
        assert_eq!(
            expanded.arguments,
            ["--token=bound", "--home=HOME-from-the-lookup"]
        );
        assert_eq!(
            expanded.environment.get("T").map(SecretValue::expose),
            Some("bound")
        );
    }

    #[test]
    fn expanding_a_remote_server_leaves_its_url_and_headers_alone() {
        let lookup = |_: &str| Some("example.com".to_string());
        let server = McpServer::remote("https://${HOST}/mcp")
            .with_header("X-Host", "${HOST}")
            .with_secret("HOST", "secret.example.com");

        assert_eq!(server.expanded(&lookup), server);
    }

    #[test]
    fn a_server_that_is_invalid_or_unnameable_is_refused() {
        assert_eq!(
            McpServer::default().refusal("remote"),
            Some(Refusal::Invalid)
        );
        assert_eq!(
            http().with_transport(McpTransport::Stdio).refusal("remote"),
            Some(Refusal::Invalid)
        );
        assert_eq!(stdio().refusal("a b"), Some(Refusal::Unnameable));
        assert_eq!(http().refusal("remote__x"), Some(Refusal::Unnameable));
        assert_eq!(stdio().refusal("appwrite"), None);
        assert_eq!(http().refusal("remote"), None);
    }

    /// A remote server is sent a value only through one of its own secrets
    /// or a default, so a reference with neither leaves the server out,
    /// whatever variable it names, rather than sending it anything.
    #[test]
    fn a_remote_server_with_a_reference_to_no_secret_and_no_default_is_refused() {
        for server in [
            http().with_header("Authorization", "Bearer ${TOKEN}"),
            McpServer::remote("https://${HOST}/mcp"),
            http().with_header("X-Stolen", "${ABNEGATE_MCP_0}"),
            http().with_header("X-Key", "${ANTHROPIC_API_KEY}"),
            http()
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("OTHER", "bound"),
        ] {
            assert_eq!(
                server.refusal("remote"),
                Some(Refusal::Unbound),
                "{server:?}"
            );
        }
        for server in [
            http()
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("TOKEN", "bound"),
            http().with_header("X-Key", "${ANTHROPIC_API_KEY:-none}"),
            McpServer::remote("https://${ABNEGATE_MCP_0:-relay.example}/mcp"),
            http().with_header("X-Literal", "ABNEGATE_MCP_0"),
            stdio().with_environment("TOKEN", "${UNSET}"),
        ] {
            assert_eq!(server.refusal("remote"), None, "{server:?}");
        }
    }

    /// A URL that resolves to nothing reaches no server, and one configured
    /// blank is refused as invalid, so one that resolves blank, through an
    /// empty default or an empty secret, is refused by the same rule rather
    /// than written for the CLI to fail on.
    #[test]
    fn a_remote_url_that_resolves_to_nothing_is_refused_like_a_blank_one() {
        for server in [
            McpServer::remote("${HOST:-}"),
            McpServer::remote("${HOST:-  }"),
            McpServer::remote("${HOST}").with_secret("HOST", ""),
            McpServer::remote(" ${HOST} ${PATH:-}").with_secret("HOST", ""),
        ] {
            assert_eq!(
                server.refusal("remote"),
                Some(Refusal::Invalid),
                "{server:?}"
            );
        }
        for server in [
            McpServer::remote("https://${HOST:-}mcp.example.com/mcp"),
            McpServer::remote("${HOST}").with_secret("HOST", "https://mcp.example.com/mcp"),
        ] {
            assert_eq!(server.refusal("remote"), None, "{server:?}");
        }
    }

    /// Claude Code expands a remote server's header values once when it
    /// reads the file and again when it connects, so `${` in a value resolved
    /// here would be read as a reference of its own; and it never expands a
    /// header's name, which would reach the server as written.
    #[test]
    fn a_remote_server_left_holding_text_the_cli_would_expand_is_refused() {
        for server in [
            http()
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("TOKEN", "${GITHUB_TOKEN}"),
            http().with_header("X-Key", "${KEY:-${GITHUB_TOKEN}}"),
            McpServer::remote("https://example.com/${1BAD}"),
            http().with_header("X-Literal", "${"),
        ] {
            assert_eq!(
                server.refusal("remote"),
                Some(Refusal::Expandable),
                "{server:?}"
            );
        }
        assert_eq!(
            http()
                .with_header("X-${TOKEN}", "value")
                .with_secret("TOKEN", "bound")
                .refusal("remote"),
            Some(Refusal::HeaderName)
        );
    }

    fn header(endpoint: &McpEndpoint, name: &str) -> String {
        endpoint.headers[name].expose().to_string()
    }

    #[test]
    fn an_endpoint_resolves_its_url_and_headers_against_bound_secrets() {
        let endpoint = McpServer::remote("https://${HOST}/mcp")
            .with_header("Authorization", "Bearer ${TOKEN}")
            .with_header("X-Literal", "plain")
            .with_secret("HOST", "mcp.example.com")
            .with_secret("TOKEN", "bound")
            .endpoint()
            .expect("a remote server with bound references has an endpoint");
        assert_eq!(endpoint.url, "https://mcp.example.com/mcp");
        assert_eq!(header(&endpoint, "Authorization"), "Bearer bound");
        assert_eq!(header(&endpoint, "X-Literal"), "plain");
        assert_eq!(endpoint.headers.len(), 2, "{endpoint:?}");
    }

    #[test]
    fn an_endpoint_reference_with_no_secret_takes_its_default() {
        let endpoint = McpServer::remote("https://${HOST:-fallback.example}/mcp")
            .with_header("X-Team", "${TEAM:-core}")
            .endpoint()
            .expect("references with defaults resolve");
        assert_eq!(endpoint.url, "https://fallback.example/mcp");
        assert_eq!(header(&endpoint, "X-Team"), "core");
    }

    #[test]
    fn an_endpoint_with_an_unbound_reference_is_none() {
        for server in [
            McpServer::remote("https://${HOST}/mcp"),
            http().with_header("Authorization", "Bearer ${TOKEN}"),
            http()
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("OTHER", "bound"),
        ] {
            assert_eq!(server.endpoint(), None, "{server:?}");
        }
    }

    #[test]
    fn a_stdio_or_invalid_server_has_no_endpoint() {
        let both = McpServer {
            url: Some("https://example.com/mcp".to_string()),
            ..stdio()
        };
        for server in [
            stdio(),
            both,
            McpServer::remote("  "),
            McpServer::remote("${HOST:-}"),
            McpServer::remote("${HOST}").with_secret("HOST", " "),
            http().with_transport(McpTransport::Stdio),
        ] {
            assert_eq!(server.endpoint(), None, "{server:?}");
        }
    }

    #[test]
    fn an_endpoint_still_holding_a_reference_or_naming_one_in_a_header_is_none() {
        for server in [
            http()
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("TOKEN", "${GITHUB_TOKEN}"),
            http().with_header("X-Key", "${KEY:-${GITHUB_TOKEN}}"),
            McpServer::remote("https://example.com/${1BAD}"),
            http()
                .with_header("X-${TOKEN}", "value")
                .with_secret("TOKEN", "bound"),
        ] {
            assert_eq!(server.endpoint(), None, "{server:?}");
        }
    }

    #[test]
    fn an_endpoint_ignores_the_name_and_is_given_over_sse_too() {
        let endpoint = http()
            .with_transport(McpTransport::Sse)
            .endpoint()
            .expect("an sse server is remote");
        assert_eq!(endpoint.url, "https://example.com/mcp");
        assert!(endpoint.headers.is_empty(), "{endpoint:?}");
    }

    #[test]
    fn endpoint_never_reads_the_process_environment() {
        assert!(
            std::env::var_os("PATH").is_some(),
            "the test relies on PATH being set"
        );
        assert_eq!(http().with_header("X", "${PATH}").endpoint(), None);
    }

    #[test]
    fn endpoint_with_reads_secrets_then_the_lookup_then_the_default() {
        let lookup = |name: &str| match name {
            "BOTH" | "LOOKUP" => Some(format!("looked-up-{name}")),
            _ => None,
        };
        let endpoint = http()
            .with_header("X-Both", "${BOTH}")
            .with_header("X-Lookup", "${LOOKUP}")
            .with_header("X-Default", "${NEITHER:-fallback}")
            .with_secret("BOTH", "secret")
            .endpoint_with(&lookup)
            .expect("every reference resolves");
        assert_eq!(header(&endpoint, "X-Both"), "secret");
        assert_eq!(header(&endpoint, "X-Lookup"), "looked-up-LOOKUP");
        assert_eq!(header(&endpoint, "X-Default"), "fallback");
        assert_eq!(
            http()
                .with_header("X-Unbound", "${NEITHER}")
                .endpoint_with(&lookup),
            None
        );
        assert_eq!(
            McpServer::remote("https://${LOOKUP}/mcp")
                .endpoint_with(&lookup)
                .map(|endpoint| endpoint.url),
            Some("https://looked-up-LOOKUP/mcp".to_string())
        );
    }

    #[test]
    fn an_endpoints_debug_never_prints_a_header_secret() {
        for url in [
            "https://${USER}@example.com/mcp?key=${KEY}",
            "https://example.com/${KEY}/mcp",
        ] {
            let endpoint = McpServer::remote(url)
                .with_header("Authorization", "Bearer ${TOKEN}")
                .with_secret("TOKEN", "header-secret-7f3a")
                .with_secret("KEY", "abc123")
                .with_secret("USER", "admin:hunter2")
                .endpoint()
                .expect("every reference is bound");
            let debug = format!("{endpoint:?}");
            for secret in ["header-secret-7f3a", "abc123", "admin", "hunter2"] {
                assert!(!debug.contains(secret), "{secret} leaked in {debug}");
            }
            assert!(debug.contains("Authorization"), "{debug}");
            assert!(debug.contains("example.com/[REDACTED]"), "{debug}");
        }
    }

    /// A server that could not attach is never rendered, and one rendered
    /// anyway is sent nothing the CLI could expand.
    #[test]
    fn a_value_that_cannot_be_resolved_is_never_written_for_the_cli_to_expand() {
        let server = McpServer::remote("https://${HOST}/mcp")
            .with_header("Authorization", "Bearer ${TOKEN}")
            .with_header("X-Key", "${KEY:-${GITHUB_TOKEN}}");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders);

        assert_eq!(entry["url"], "");
        assert_eq!(entry["headers"]["Authorization"], "");
        assert_eq!(entry["headers"]["X-Key"], "");
        assert!(placeholders.environment.is_empty());
    }

    /// A secret is bound in code and never belongs in a document: one written
    /// back would put the token in a file.
    #[test]
    fn a_secret_is_never_written_read_or_printed() {
        let server = http()
            .with_header("Authorization", "Bearer ${TOKEN}")
            .with_secret("TOKEN", "bound-secret-marker");

        let written = serde_json::to_value(&server).expect("serialisable");
        let read: McpServer = serde_json::from_value(json!({
            "url": "https://example.com/mcp",
            "secrets": {"TOKEN": "from-a-document"}
        }))
        .expect("a server");

        assert!(
            !written.to_string().contains("bound-secret-marker"),
            "{written}"
        );
        assert!(written.get("secrets").is_none(), "{written}");
        assert!(read.secrets.is_empty());
        assert!(!format!("{server:?}").contains("bound-secret-marker"));
        assert!(
            !server
                .redacted()
                .to_string()
                .contains("bound-secret-marker")
        );
    }

    #[test]
    fn an_entry_never_carries_inherit_environment_or_disabled() {
        let entry = stdio()
            .inherit_environment()
            .disable()
            .entry(&mut Placeholders::under("TEST"));

        assert!(entry.get("inherit_environment").is_none(), "{entry}");
        assert!(entry.get("disabled").is_none(), "{entry}");
    }

    #[test]
    fn a_qualified_name_spells_the_server_and_tool_as_the_cli_does() {
        assert_eq!(qualified("relay", "echo"), "mcp__relay__echo");
        assert_eq!(
            qualified("relay", "list/files.all"),
            "mcp__relay__list_files_all"
        );
    }
}
