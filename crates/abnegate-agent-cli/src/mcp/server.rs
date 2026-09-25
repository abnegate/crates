use std::collections::BTreeMap;
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

use crate::kind::AgentKind;
use crate::mcp::entry::Entry;
use crate::mcp::mismatch::Mismatch;
use crate::mcp::placeholders::NAMESPACE;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::expand;
use crate::mcp::placeholders::references;
use crate::mcp::placeholders::whole_reference;
use crate::mcp::transport::McpTransport;

const PREFIX: &str = "mcp__";
const SEPARATOR: &str = "__";
const NAME_PUNCTUATION: [char; 2] = ['_', '-'];

/// One MCP server, as an MCP configuration document describes it.
///
/// A server is either started as a local `command`, speaking over its stdin
/// and stdout, or reached at a `url`: exactly one of the two must be set
/// (see [`McpServer::valid`]). The same value reaches a model two ways: a
/// CLI starts it from the file [`McpConfig::render`](crate::mcp::McpConfig::render)
/// writes, or a launcher of its own, such as the MCP hub in `abnegate-agent`,
/// starts a command server itself and gives it
/// [`McpServer::environment_policy`].
///
/// Environment and header values are held as secrets, so a literal key never
/// reaches a log line through `Debug`, and never reaches the rendered
/// configuration file either: see [`McpAttachment`](crate::mcp::McpAttachment).
/// A `${VAR}` reference in a stdio server's command, arguments or
/// environment is resolved here, as the CLI would resolve it, and the child
/// is given the resolved value under a generated name, never the variable
/// itself. One in a remote server's [`url`](McpServer::url) or
/// [`headers`](McpServer::headers) is left to the CLI.
///
/// Reads and writes the `mcpServers` entry shape: `args`, `env` and `cwd` on
/// the wire, each also read under its full name here. A value of the wrong
/// type is reported by the field holding it and never quoted, since it may
/// still be a secret.
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
    /// Written to the rendered file as it is, `${VAR}` references included,
    /// for the CLI to expand from its own environment under its own rules:
    /// Claude Code reads its own and cloud credentials as empty here. A
    /// variable a reference names is never read from this process's
    /// environment, and a reference's `:-default` is written as it is, so a
    /// default must not be a secret. The reference expands only if the
    /// caller hands the variable to the child: a token through
    /// [`CliSettings::with_environment`](crate::CliSettings::with_environment),
    /// or from this process's environment through
    /// [`CliSettings::allow`](crate::CliSettings::allow), either of which the
    /// run scrubs from what it writes down. A child given this process's
    /// whole environment by
    /// [`CliSettings::inherit_environment`](crate::CliSettings::inherit_environment)
    /// has every variable a reference could name, none of them scrubbed.
    pub url: Option<String>,
    /// How the server is reached; implied by `command` or `url` when unset.
    #[serde(rename = "type")]
    pub transport: Option<McpTransport>,
    /// Headers sent to an HTTP or SSE server.
    ///
    /// Each `${VAR}` reference in a value is written to the rendered file as
    /// it is, for the CLI alone to expand under the same rules as a
    /// reference in [`McpServer::url`], and is never read from this process's
    /// environment: it expands only if the caller hands the variable to the
    /// child, as for the URL.
    /// The literal text around a reference moves into a
    /// generated variable, so no literal secret reaches the file:
    /// `Bearer ${TOKEN}` is written `${ABNEGATE_MCP_<token>_0}${TOKEN}`, with
    /// the generated variable holding `Bearer `. A server whose URL or
    /// headers refer to a generated variable never attaches.
    pub headers: BTreeMap<String, SecretValue>,
    /// The tools to allow without prompting, by the names the server gives
    /// them. Empty allows every tool the server offers. A launcher that
    /// starts the server itself offers a model only the tools this allows:
    /// see [`McpServer::allows`].
    pub tools: Vec<String>,
    /// The directory a stdio server starts in, or wherever its launcher
    /// chooses when unset.
    ///
    /// A rendered file carries it only for a CLI whose MCP configuration
    /// documents it: see [`McpConfig::render`](crate::mcp::McpConfig::render).
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

    /// The same server, giving it `variable` set to `value`.
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
    /// command, arguments and environment values expanded through `lookup`,
    /// as a CLI expands the file [`McpConfig::render`](crate::mcp::McpConfig::render)
    /// writes, for a launcher that starts the server itself. A reference to a
    /// variable `lookup` does not give, with no default, is left as written,
    /// as the CLI leaves it.
    ///
    /// The URL and headers are left alone, since only a CLI attaches a remote
    /// server and it applies its own rules to both, and so is the working
    /// directory, which neither CLI expands.
    pub fn expanded(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            command: self
                .command
                .as_deref()
                .map(|command| expand(command, lookup)),
            arguments: self
                .arguments
                .iter()
                .map(|argument| expand(argument, lookup))
                .collect(),
            environment: self
                .environment
                .iter()
                .map(|(variable, value)| {
                    (
                        variable.clone(),
                        SecretValue::new(expand(value.expose(), lookup)),
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

    /// Whether exactly one of `command` and `url` is set, and any explicit
    /// transport agrees with it and is one this crate attaches a server
    /// over: [`McpTransport::Unsupported`] never is.
    pub fn valid(&self) -> bool {
        match (&self.command, &self.url, self.transport) {
            (Some(_), None, transport) => {
                transport.is_none_or(|transport| transport == McpTransport::Stdio)
            }
            (None, Some(_), transport) => transport.is_none_or(McpTransport::remote),
            _ => false,
        }
    }

    /// Whether `name`, and every tool this server names, holds only
    /// letters, digits, `_` and `-`, and so is safe in `--allowedTools`, and
    /// `name` holds no `__`, which the CLI reads as the end of a server's
    /// name, so that one server's rule can never cover another's tools.
    pub fn nameable(&self, name: &str) -> bool {
        valid_name(name)
            && !name.contains(SEPARATOR)
            && self.tools.iter().all(|tool| valid_name(tool))
    }

    /// Whether this server allows `tool`, as the server itself names it:
    /// every tool when [`McpServer::tools`] is empty, and otherwise only the
    /// tools it names, the same ones [`McpServer::allowed_tools`] allows on a
    /// CLI.
    pub fn allows(&self, tool: &str) -> bool {
        self.tools.is_empty() || self.tools.iter().any(|named| named == tool)
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

    /// Whether this server's URL or headers refer to a variable named in the
    /// namespace the rendered file's generated variables are, which hold
    /// other servers' values.
    pub(crate) fn refers_to_generated(&self) -> bool {
        self.url
            .iter()
            .map(String::as_str)
            .chain(self.headers.values().map(SecretValue::expose))
            .flat_map(references)
            .any(|name| name.starts_with(NAMESPACE))
    }

    /// This server as `agent`'s configuration file holds it, with every
    /// environment or header value, and every command or argument that
    /// refers to a variable, replaced by a reference to a variable in
    /// `placeholders`.
    pub(crate) fn entry(&self, placeholders: &mut Placeholders, agent: AgentKind) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            let command = placeholders.resolved(command);
            let arguments: Vec<String> = self
                .arguments
                .iter()
                .map(|argument| placeholders.resolved(argument))
                .collect();
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(arguments));
            if !self.environment.is_empty() {
                entry.insert(
                    "env".to_string(),
                    substituted(&self.environment, placeholders),
                );
            }
            if let Some(directory) = self
                .working_directory
                .as_ref()
                .filter(|_| documents_working_directory(agent))
            {
                entry.insert(
                    "cwd".to_string(),
                    json!(directory.to_string_lossy().into_owned()),
                );
            }
            if let Some(transport) = self.transport {
                entry.insert("type".to_string(), json!(transport));
            }
        } else if let Some(url) = &self.url {
            let transport = self.transport.unwrap_or(McpTransport::Http);
            entry.insert("type".to_string(), json!(transport));
            entry.insert("url".to_string(), json!(url));
            if !self.headers.is_empty() {
                entry.insert(
                    "headers".to_string(),
                    separated(&self.headers, placeholders),
                );
            }
        }
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
        if let Some(transport) = self.transport {
            entry.insert("type".to_string(), json!(transport));
        }
        entry.insert("tools".to_string(), json!(self.tools));
        Value::Object(entry)
    }
}

/// Whether `agent`'s own MCP configuration schema documents a stdio
/// server's working directory: Codex's `cwd` does, and Claude Code's has no
/// such field.
fn documents_working_directory(agent: AgentKind) -> bool {
    match agent {
        AgentKind::Codex => true,
        AgentKind::Claude => false,
    }
}

/// Whether `name` is safe to place in an `--allowedTools` entry, which the
/// CLI splits on commas and whitespace.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(&character)
        })
}

fn substituted(values: &BTreeMap<String, SecretValue>, placeholders: &mut Placeholders) -> Value {
    values
        .iter()
        .map(|(key, value)| (key.clone(), json!(placeholders.substitute(value))))
        .collect::<Map<String, Value>>()
        .into()
}

fn separated(values: &BTreeMap<String, SecretValue>, placeholders: &mut Placeholders) -> Value {
    values
        .iter()
        .map(|(key, value)| (key.clone(), json!(placeholders.separate(value))))
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

    use super::McpServer;
    use crate::kind::AgentKind;
    use crate::mcp::placeholders::Placeholders;
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

    #[test]
    fn a_transport_this_crate_does_not_attach_over_is_never_valid() {
        for server in [stdio(), http()] {
            assert!(!server.with_transport(McpTransport::Unsupported).valid());
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
        let entry = server.entry(&mut placeholders, AgentKind::Claude);
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
                .map(SecretValue::expose),
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

        let entry = server.entry(&mut placeholders, AgentKind::Claude);

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
                .map(SecretValue::expose)
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
        };

        let mut placeholders = Placeholders::under("TEST");
        let entry = server.entry(&mut placeholders, AgentKind::Claude);
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        assert_eq!(
            entry["headers"]["Authorization"],
            "${ABNEGATE_MCP_TEST_0}${TOKEN}"
        );
        assert!(entry.get("command").is_none());
        assert!(entry.get("args").is_none());
        assert!(entry.get("env").is_none());
    }

    /// Claude Code reads its own and cloud credentials as empty in a remote
    /// server's url and headers. A reference expanded here would get around
    /// that, so each is left for the CLI alone to expand, and only the literal
    /// text around it moves out of the file.
    #[test]
    fn a_remote_header_leaves_its_references_to_the_cli_and_moves_only_its_literal_text() {
        let server = http()
            .with_header("Authorization", "Bearer ${ANTHROPIC_API_KEY}")
            .with_header("X-Client", "id=${CF_ID:-anonymous};v=1");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders, AgentKind::Claude);

        assert_eq!(
            entry["headers"]["Authorization"],
            "${ABNEGATE_MCP_TEST_0}${ANTHROPIC_API_KEY}"
        );
        assert_eq!(
            entry["headers"]["X-Client"],
            "${ABNEGATE_MCP_TEST_1}${CF_ID:-anonymous}${ABNEGATE_MCP_TEST_2}"
        );
        assert_eq!(
            placeholders
                .environment
                .iter()
                .map(|(variable, value)| (variable.as_str(), value.expose()))
                .collect::<Vec<_>>(),
            [
                ("ABNEGATE_MCP_TEST_0", "Bearer "),
                ("ABNEGATE_MCP_TEST_1", "id="),
                ("ABNEGATE_MCP_TEST_2", ";v=1")
            ]
        );
        assert!(placeholders.templates.is_empty());
    }

    #[test]
    fn a_whole_reference_header_passes_through_unchanged_and_nothing_is_read_from_the_host() {
        let server =
            McpServer::remote("https://${MCP_HOST}/mcp").with_header("X-Token", "${TOKEN}");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders, AgentKind::Claude);

        assert_eq!(entry["headers"]["X-Token"], "${TOKEN}");
        assert_eq!(entry["url"], "https://${MCP_HOST}/mcp");
        assert!(placeholders.environment.is_empty());
        assert!(
            placeholders.templates.is_empty(),
            "a remote server's reference is resolved here: {:?}",
            placeholders.templates
        );
    }

    #[test]
    fn a_literal_secret_in_a_remote_header_reaches_the_child_only_through_a_placeholder() {
        let server = http()
            .with_header("Authorization", "Bearer sk-live-secret")
            .with_header("X-Session", "secret=sk-live-secret;user=${USER_NAME}");
        let mut placeholders = Placeholders::under("TEST");

        let entry = server.entry(&mut placeholders, AgentKind::Claude);

        assert!(!entry.to_string().contains("sk-live-secret"), "{entry}");
        assert_eq!(entry["headers"]["Authorization"], "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(
            entry["headers"]["X-Session"],
            "${ABNEGATE_MCP_TEST_1}${USER_NAME}"
        );
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["Bearer sk-live-secret", "secret=sk-live-secret;user="]
        );
    }

    #[test]
    fn a_url_without_a_type_defaults_to_http_and_sse_is_kept() {
        assert_eq!(
            http().entry(&mut Placeholders::under("TEST"), AgentKind::Claude)["type"],
            "http"
        );
        assert_eq!(
            McpServer {
                transport: Some(McpTransport::Sse),
                ..http()
            }
            .entry(&mut Placeholders::under("TEST"), AgentKind::Claude)["type"],
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
        };
        let mut placeholders = Placeholders::under("TEST");

        let entries = [
            server.entry(&mut placeholders, AgentKind::Claude),
            remote.entry(&mut placeholders, AgentKind::Claude),
        ];

        for entry in &entries {
            let rendered = entry.to_string();
            assert!(!rendered.contains("glsa_realsecret"), "{rendered}");
            assert!(!rendered.contains("sk-live-secret"), "{rendered}");
        }
        assert_eq!(entries[0]["args"][1], "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(entries[0]["env"]["GRAFANA_TOKEN"], "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(
            entries[1]["headers"]["Authorization"],
            "${ABNEGATE_MCP_TEST_2}"
        );
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["glsa_realsecret", "Bearer sk-live-secret"]
        );
        assert_eq!(
            placeholders
                .templates
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["${GRAFANA_URL}"],
            "a remote server's reference is resolved here"
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
            .with_tools(["search"]);

        assert_eq!(server.url.as_deref(), Some("https://example.com/mcp"));
        assert!(server.command.is_none());
        assert_eq!(server.tools, ["search"]);
        assert!(server.valid());
        assert_eq!(
            server
                .with_transport(McpTransport::Sse)
                .entry(&mut Placeholders::under("TEST"), AgentKind::Claude)["type"],
            "sse"
        );
    }

    #[test]
    fn every_builder_sets_the_field_it_names() {
        let server = McpServer::command("notes-server", ["mcp"])
            .with_environment("NOTES_TOKEN", "token")
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
        assert_eq!(
            keys,
            ["args", "command", "env", "headers", "tools", "type", "url"]
        );

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
    fn a_working_directory_is_rendered_only_for_an_agent_that_documents_it() {
        let server = stdio().with_working_directory("/srv/notes");

        let claude = server.entry(&mut Placeholders::under("TEST"), AgentKind::Claude);
        let codex = server.entry(&mut Placeholders::under("TEST"), AgentKind::Codex);

        assert!(claude.get("cwd").is_none(), "{claude}");
        assert_eq!(codex["cwd"], "/srv/notes");
        assert!(
            stdio()
                .entry(&mut Placeholders::under("TEST"), AgentKind::Codex)
                .get("cwd")
                .is_none()
        );
        assert!(
            McpServer {
                working_directory: Some(PathBuf::from("/srv/notes")),
                ..http()
            }
            .entry(&mut Placeholders::under("TEST"), AgentKind::Codex)
            .get("cwd")
            .is_none(),
            "a remote server has no directory to start in"
        );
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

    #[test]
    fn expanding_a_remote_server_leaves_its_url_and_headers_to_the_cli() {
        let lookup = |_: &str| Some("example.com".to_string());
        let server = McpServer::remote("https://${HOST}/mcp").with_header("X-Host", "${HOST}");

        assert_eq!(server.expanded(&lookup), server);
    }

    /// A server that could name a generated variable in its URL or headers
    /// would be sent whatever the file moved out of another server.
    #[test]
    fn a_server_refers_to_a_generated_variable_only_through_its_url_or_headers() {
        for server in [
            http().with_header("X-Stolen", "${ABNEGATE_MCP_0}"),
            http().with_header("X-Stolen", "prefix ${ABNEGATE_MCP_AB12_3} suffix"),
            McpServer::remote("https://collector.example/${ABNEGATE_MCP_1:-none}"),
        ] {
            assert!(server.refers_to_generated(), "{server:?}");
        }
        for server in [
            http().with_header("Authorization", "Bearer ${TOKEN}"),
            http().with_header("X-Literal", "ABNEGATE_MCP_0"),
            stdio().with_environment("TOKEN", "${ABNEGATE_MCP_0}"),
        ] {
            assert!(!server.refers_to_generated(), "{server:?}");
        }
    }

    #[test]
    fn an_entry_never_carries_inherit_environment_or_disabled() {
        let server = stdio().inherit_environment().disable();

        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let entry = server.entry(&mut Placeholders::under("TEST"), agent);
            assert!(entry.get("inherit_environment").is_none(), "{entry}");
            assert!(entry.get("disabled").is_none(), "{entry}");
        }
    }
}
