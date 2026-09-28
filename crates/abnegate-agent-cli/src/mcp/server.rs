use std::collections::BTreeMap;
use std::iter;
use std::path::PathBuf;

use abnegate_exec::EnvironmentPolicy;
use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::mcp::entry::Entry;
use crate::mcp::mismatch::Mismatch;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::expand;
use crate::mcp::placeholders::refers;
use crate::mcp::placeholders::resolvable;
use crate::mcp::placeholders::whole_reference;
use crate::mcp::refusal::Refusal;
use crate::mcp::segment::OPENING;
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
/// [`secrets`](McpServer::secrets). A value of the wrong type is reported by
/// the field holding it and never quoted, since it may still be a secret.
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
    /// run attaches the server, against the server's own
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
    /// the server offers. A launcher that starts the server itself offers a
    /// model only the tools this allows: see [`McpServer::allows`].
    pub tools: Vec<String>,
    /// The directory a stdio server starts in, or wherever its launcher
    /// chooses when unset.
    ///
    /// A rendered file never carries it, since Claude Code's MCP
    /// configuration has no such field: only a launcher that starts the
    /// server itself, such as the MCP hub in `abnegate-agent`, honours it.
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
    /// The URL and headers are left alone, since only a CLI's run attaches a
    /// remote server, resolving both against its
    /// [`secrets`](McpServer::secrets), and so is the working directory,
    /// which neither CLI expands.
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
        valid_name(name)
            && !name.contains(SEPARATOR)
            && !name.ends_with(UNDERSCORE)
            && self.tools.iter().all(|tool| valid_name(tool))
    }

    /// Whether this server allows `tool`, as the server itself names it:
    /// every tool when [`McpServer::tools`] is empty, and otherwise only one
    /// whose name, as the CLI gives it, the list holds: the same tools
    /// [`McpServer::allowed_tools`] allows on a CLI.
    pub fn allows(&self, tool: &str) -> bool {
        let named = cli_name(tool);
        self.tools.is_empty() || self.tools.contains(&named)
    }

    /// The `--allowedTools` entries for this server under `name`.
    pub fn allowed_tools(&self, name: &str) -> Vec<String> {
        if self.tools.is_empty() {
            return vec![format!("{PREFIX}{name}")];
        }
        self.scoped_tools(name)
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
        if !self.valid() {
            return Some(Refusal::Invalid);
        }
        if !self.nameable(name) {
            return Some(Refusal::Unnameable);
        }
        let Some(url) = &self.url else {
            return None;
        };
        if self.headers.keys().any(|header| header.contains(OPENING)) {
            return Some(Refusal::HeaderName);
        }
        let resolved: Option<Vec<String>> = iter::once(url.as_str())
            .chain(self.headers.values().map(SecretValue::expose))
            .map(|value| self.resolved(value))
            .collect();
        match resolved {
            None => Some(Refusal::Unbound),
            Some(values) if values.iter().any(|value| value.contains(OPENING)) => {
                Some(Refusal::Expandable)
            }
            Some(_) => None,
        }
    }

    /// `value`, from this server's URL or headers, with every reference
    /// resolved against the server's own [`secrets`](McpServer::secrets), one
    /// with no secret taking its default, or none when such a reference has
    /// no default.
    fn resolved(&self, value: &str) -> Option<String> {
        resolvable(value, &|name| self.secrets.contains_key(name)).then(|| {
            expand(value, &|name| {
                self.secrets
                    .get(name)
                    .map(|secret| secret.expose().to_string())
            })
        })
    }

    /// `value` as the server is sent it: resolved, or nothing at all when it
    /// cannot be, so a server that could not attach is never sent anything
    /// for the CLI to expand.
    fn sent(&self, value: &str) -> String {
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
    /// no default, which names a secret without holding one, and anything
    /// credential shaped in the arguments or the URL redacted.
    pub(crate) fn redacted(&self) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            let arguments: Vec<_> = self
                .arguments
                .iter()
                .map(|argument| redact(argument))
                .collect();
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(arguments));
            if !self.environment.is_empty() {
                entry.insert("env".to_string(), masked(&self.environment));
            }
        } else if let Some(url) = &self.url {
            entry.insert("url".to_string(), json!(redact(url)));
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

    use abnegate_exec::DEFAULT_ENVIRONMENT;
    use abnegate_secret::SecretValue;
    use serde_json::json;

    use serde_json::Value;

    use super::McpServer;
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
}
