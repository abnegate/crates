use std::collections::BTreeMap;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error;
use serde::ser::SerializeMap;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::kind::AgentKind;
use crate::mcp::attachment::McpAttachment;
use crate::mcp::config_error::McpConfigError;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::server::McpServer;

/// The prefix [`McpConfig::from_environment`] is given by an application
/// with no prefix of its own: `ABNEGATE_MCP_SERVERS`,
/// `~/.abnegate/mcp.json` and so on.
pub const DEFAULT_PREFIX: &str = "ABNEGATE";

const SERVERS: &str = "mcpServers";
const SERVERS_ALIAS: &str = "servers";
const FILE_PREFIX: &str = "mcp-";
const FILE_SUFFIX: &str = ".json";
const ENABLED_VARIABLE: &str = "ENABLED";
const SERVERS_VARIABLE: &str = "SERVERS";
const CONFIG_VARIABLE: &str = "CONFIG";
const AUTO_CONNECT_VARIABLE: &str = "AUTO_CONNECT";
const CONFIG_FILE: &str = "mcp.json";

/// The MCP servers a run attaches, keyed by the name the agent knows each by.
///
/// It reads a `{"mcpServers": {...}}` document, the shape Claude Code and
/// Cursor use, a `{"servers": {...}}` one, or a bare map of servers, and
/// writes the first. [`McpConfig::from_environment`] finds one through an
/// application's own variables.
///
/// A [disabled](McpServer::disabled) server stays here, as configured, and
/// every method that attaches, renders, launches or allows servers leaves it
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpConfig {
    /// Every configured server, disabled ones included.
    pub servers: BTreeMap<String, McpServer>,
    /// Whether [`McpConfig::fallback`] may attach a server when none is
    /// enabled. It is not part of the document: on unless
    /// `{PREFIX}_MCP_AUTO_CONNECT` turns it off.
    pub auto_connect: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl Serialize for McpConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut document = serializer.serialize_map(Some(1))?;
        document.serialize_entry(SERVERS, &self.servers)?;
        document.end()
    }
}

impl<'de> Deserialize<'de> for McpConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let document = Value::deserialize(deserializer)?;
        Self::from_value(&document).map_err(D::Error::custom)
    }
}

impl McpConfig {
    /// Exactly `servers`, with [`McpConfig::auto_connect`] on.
    pub fn new(servers: BTreeMap<String, McpServer>) -> Self {
        Self {
            servers,
            auto_connect: true,
        }
    }

    /// The same configuration with `server` under `name`, in place of any
    /// server already there.
    pub fn with_server(mut self, name: impl Into<String>, server: McpServer) -> Self {
        self.servers.insert(name.into(), server);
        self
    }

    /// Whether no server is enabled.
    pub fn is_empty(&self) -> bool {
        self.enabled().next().is_none()
    }

    /// Every server not [disabled](McpServer::disabled), in name order.
    pub fn enabled(&self) -> impl Iterator<Item = (&str, &McpServer)> {
        self.servers
            .iter()
            .filter(|(_, server)| !server.disabled)
            .map(|(name, server)| (name.as_str(), server))
    }

    /// Load from the environment under an application's own prefix, all of
    /// it optional: `ACME` reads
    ///
    /// - `ACME_MCP_ENABLED`: the master switch, on by default. Off, nothing
    ///   is configured and [`McpConfig::auto_connect`] is off too.
    /// - `ACME_MCP_SERVERS`: an inline document.
    /// - `ACME_MCP_CONFIG`: the path to a document, read when no inline one
    ///   is set; `~/.acme/mcp.json` is read when neither is.
    /// - `ACME_MCP_AUTO_CONNECT`: [`McpConfig::auto_connect`], on by default.
    ///
    /// [`DEFAULT_PREFIX`] is the prefix for an application with none of its
    /// own. This never fails: a document that cannot be read is logged and
    /// skipped, leaving no server configured.
    pub fn from_environment(prefix: &str) -> Self {
        Self::load(
            prefix,
            |variable| std::env::var(variable).ok(),
            dirs::home_dir(),
        )
    }

    /// [`McpConfig::from_environment`], reading variables through `read`
    /// and the default document from beneath `home`.
    fn load(prefix: &str, read: impl Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Self {
        let flag =
            |name: &str| read(&variable(prefix, name)).is_none_or(|value| parse_flag(&value, true));
        if !flag(ENABLED_VARIABLE) {
            return Self {
                servers: BTreeMap::new(),
                auto_connect: false,
            };
        }
        Self {
            auto_connect: flag(AUTO_CONNECT_VARIABLE),
            ..Self::configured(prefix, &read, home)
        }
    }

    fn configured(
        prefix: &str,
        read: &impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Self {
        let servers = variable(prefix, SERVERS_VARIABLE);
        if let Some(raw) = read(&servers).filter(|raw| !raw.trim().is_empty()) {
            return Self::from_json_str(&raw).unwrap_or_else(|error| {
                tracing::warn!(variable = %servers, error = %error, "MCP servers are invalid; ignoring");
                Self::default()
            });
        }

        let config = variable(prefix, CONFIG_VARIABLE);
        if let Some(path) = read(&config).filter(|path| !path.trim().is_empty()) {
            return Self::from_file(Path::new(&path)).unwrap_or_else(|error| {
                tracing::warn!(
                    variable = %config,
                    path = %path,
                    error = %error,
                    "MCP config could not be loaded; ignoring"
                );
                Self::default()
            });
        }

        let Some(path) = home
            .map(|home| default_path(&home, prefix))
            .filter(|path| path.is_file())
        else {
            return Self::default();
        };
        Self::from_file(&path).unwrap_or_else(|error| {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "default MCP config could not be loaded; ignoring"
            );
            Self::default()
        })
    }

    /// Parse a document: see [`McpConfig::from_value`].
    pub fn from_json_str(raw: &str) -> Result<Self, McpConfigError> {
        let value: Value = serde_json::from_str(raw)?;
        Self::from_value(&value)
    }

    /// Read and parse the document at `path`: see [`McpConfig::from_value`].
    pub fn from_file(path: &Path) -> Result<Self, McpConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| McpConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&raw)
    }

    /// Parse `{"mcpServers": {...}}`, `{"servers": {...}}` or a bare map of
    /// servers, with [`McpConfig::auto_connect`] on.
    ///
    /// `servers` is taken as the wrapper only when every value under it is
    /// an object, so a bare map may still name a server `servers`. Each
    /// server is read on its own: one whose entry cannot be read is left out
    /// with a warning carrying its [`McpConfigError::Server`], and the rest
    /// load. A server that is neither a command nor a URL, is both, or names
    /// a transport this crate does not attach over is kept here and left out
    /// wherever servers attach. Only a document of none of these shapes is
    /// an error.
    pub fn from_value(value: &Value) -> Result<Self, McpConfigError> {
        let servers = server_map(value)?
            .iter()
            .filter_map(|(name, entry)| match Self::server(name, entry) {
                Ok(server) => Some((name.clone(), server)),
                Err(error) => {
                    tracing::warn!(%error, "skipping an MCP server that could not be read");
                    None
                }
            })
            .collect();
        Ok(Self::new(servers))
    }

    /// The server `entry` describes under `name`, or why it describes none.
    fn server(name: &str, entry: &Value) -> Result<McpServer, McpConfigError> {
        McpServer::read(entry).map_err(|mismatch| McpConfigError::server(name, mismatch))
    }

    /// Attach `server` under `name` when no server is enabled,
    /// [`McpConfig::auto_connect`] is on, the server is
    /// [valid](McpServer::valid), and its command, if it has one, is on
    /// `PATH`.
    ///
    /// For an application that ships a companion server of its own and
    /// wants it attached unless the user has configured servers themselves:
    ///
    /// ```
    /// use abnegate_agent_cli::McpConfig;
    /// use abnegate_agent_cli::McpServer;
    ///
    /// let config = McpConfig::from_environment("ACME")
    ///     .fallback("notes", McpServer::command("notes-server", ["mcp"]));
    ///
    /// for (name, server) in config.enabled() {
    ///     println!("{name}: {:?}", server.command);
    /// }
    /// ```
    pub fn fallback(mut self, name: impl Into<String>, server: McpServer) -> Self {
        let available = server.command.as_deref().is_none_or(command_on_path);
        if self.auto_connect && self.is_empty() && server.valid() && available {
            self.servers.insert(name.into(), server);
        }
        self
    }

    /// The servers that will actually attach: those enabled, with a
    /// [valid](McpServer::valid) transport, which a strict CLI would
    /// otherwise reject along with every other server, a name and tool
    /// names safe to place in `--allowedTools`, which the CLI splits on
    /// commas and whitespace, so a name holding either could allow a tool
    /// nobody named, and no reference in the URL or headers to a variable
    /// [`McpConfig::render`] generates, which holds another server's value.
    pub fn attachable(&self) -> impl Iterator<Item = (&str, &McpServer)> {
        self.enabled().filter(|(name, server)| {
            server.valid() && server.nameable(name) && !server.refers_to_generated()
        })
    }

    /// Write the attachable servers to a private temporary file for
    /// `agent`'s `--mcp-config`, or `None` when there are none, with what
    /// the child needs in its environment for the file to resolve.
    ///
    /// The file carries a server's
    /// [working directory](McpServer::working_directory) only for an agent
    /// whose own MCP configuration documents one (Codex's `cwd`; Claude
    /// Code's has none), and never
    /// [`inherit_environment`](McpServer::inherit_environment).
    pub fn render(&self, agent: AgentKind) -> io::Result<Option<McpAttachment>> {
        for (name, server) in self.enabled() {
            if !server.valid() {
                tracing::warn!(
                    server = %name,
                    "skipping an MCP server: set exactly one of `command` and `url`, and a `type`, if any, of `stdio`, `http` or `sse` that matches it"
                );
            } else if !server.nameable(name) {
                tracing::warn!(
                    server = %name,
                    "skipping an MCP server: its name and tool names may hold only letters, digits, `_` and `-`"
                );
            } else if server.refers_to_generated() {
                tracing::warn!(
                    server = %name,
                    "skipping an MCP server: its URL or headers refer to a variable generated for another server"
                );
            }
        }

        let mut placeholders = Placeholders::new()?;
        let servers: Map<String, Value> = self
            .attachable()
            .map(|(name, server)| (name.to_string(), server.entry(&mut placeholders, agent)))
            .collect();
        if servers.is_empty() {
            return Ok(None);
        }

        let document = json!({ SERVERS: servers });
        let bytes = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
        let mut file = tempfile::Builder::new()
            .prefix(FILE_PREFIX)
            .suffix(FILE_SUFFIX)
            .tempfile()?;
        file.as_file_mut().write_all(&bytes)?;
        file.as_file_mut().flush()?;
        Ok(Some(placeholders.attachment(file)))
    }

    /// The attachable servers as [`McpConfig::render`] writes them, safe for
    /// a log line.
    pub fn redacted(&self) -> Value {
        let servers: Map<String, Value> = self
            .attachable()
            .map(|(name, server)| (name.to_string(), server.redacted()))
            .collect();
        json!({ SERVERS: servers })
    }

    /// The `--allowedTools` entries for every attachable server.
    pub fn allowed_tools(&self) -> Vec<String> {
        self.attachable()
            .flat_map(|(name, server)| server.allowed_tools(name))
            .collect()
    }

    /// The `--allowedTools` entries for the tools each attachable server
    /// names, leaving out every server that names none.
    pub fn scoped_tools(&self) -> Vec<String> {
        self.attachable()
            .flat_map(|(name, server)| server.scoped_tools(name))
            .collect()
    }
}

/// The object of servers in `document`: under `mcpServers`, under `servers`
/// when every value there is an object, or the whole document when every
/// value in it is.
fn server_map(document: &Value) -> Result<&Map<String, Value>, McpConfigError> {
    let invalid =
        || McpConfigError::Invalid("expected mcpServers, servers or a map of servers".to_string());
    let object = document.as_object().ok_or_else(invalid)?;
    if let Some(servers) = object.get(SERVERS) {
        return servers
            .as_object()
            .ok_or_else(|| McpConfigError::Invalid(format!("{SERVERS} must be an object")));
    }
    if let Some(servers) = object
        .get(SERVERS_ALIAS)
        .and_then(Value::as_object)
        .filter(|servers| every_value_an_object(servers))
    {
        return Ok(servers);
    }
    if every_value_an_object(object) {
        return Ok(object);
    }
    Err(invalid())
}

fn every_value_an_object(map: &Map<String, Value>) -> bool {
    map.values().all(Value::is_object)
}

fn variable(prefix: &str, name: &str) -> String {
    format!("{prefix}_MCP_{name}")
}

fn default_path(home: &Path, prefix: &str) -> PathBuf {
    home.join(format!(".{}", prefix.to_ascii_lowercase()))
        .join(CONFIG_FILE)
}

fn parse_flag(value: &str, default: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

/// Whether `name` resolves on `PATH`.
fn command_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        directory.join(name).is_file()
            || (cfg!(windows) && directory.join(format!("{name}.exe")).is_file())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::error::Error;
    use std::ffi::OsString;
    use std::io::Read;
    use std::path::Path;
    use std::path::PathBuf;

    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;
    use serde_json::Value;
    use tempfile::NamedTempFile;
    use tokio::process::Command;

    use super::AUTO_CONNECT_VARIABLE;
    use super::CONFIG_VARIABLE;
    use super::DEFAULT_PREFIX;
    use super::ENABLED_VARIABLE;
    use super::McpConfig;
    use super::SERVERS_VARIABLE;
    use super::default_path;
    use super::parse_flag;
    use super::variable;
    use crate::environment::Environment;
    use crate::kind::AgentKind;
    use crate::mcp::attachment::McpAttachment;
    use crate::mcp::config_error::McpConfigError;
    use crate::mcp::server::McpServer;
    use crate::mcp::transport::McpTransport;
    use crate::settings::CliSettings;
    use crate::test_support::captured_logs;

    fn rendered(config: &McpConfig) -> McpAttachment {
        config
            .render(AgentKind::Claude)
            .expect("rendered")
            .expect("an attachment")
    }

    fn read(file: &NamedTempFile) -> Value {
        let mut contents = String::new();
        file.reopen()
            .expect("the rendered file")
            .read_to_string(&mut contents)
            .expect("readable");
        serde_json::from_str(&contents).expect("JSON")
    }

    fn appwrite() -> McpServer {
        McpServer {
            command: Some("uvx".to_string()),
            arguments: vec!["mcp-server-appwrite".to_string()],
            environment: [(
                "APPWRITE_API_KEY".to_string(),
                SecretValue::new("${APPWRITE_API_KEY}"),
            )]
            .into(),
            ..McpServer::default()
        }
    }

    fn broken() -> McpServer {
        McpServer {
            command: Some("uvx".to_string()),
            url: Some("https://example.com/mcp".to_string()),
            ..McpServer::default()
        }
    }

    #[test]
    fn a_rendered_file_holds_every_attachable_server_and_only_its_owner_can_read_it() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server(
                "remote",
                McpServer {
                    url: Some("https://example.com/mcp".to_string()),
                    transport: Some(McpTransport::Http),
                    ..McpServer::default()
                },
            );

        let attachment = rendered(&config);
        let file = &attachment.file;
        let document = read(file);

        let appwrite = &document["mcpServers"]["appwrite"];
        assert_eq!(appwrite["command"], "uvx");
        assert_eq!(appwrite["args"][0], "mcp-server-appwrite");
        let variable = appwrite["env"]["APPWRITE_API_KEY"]
            .as_str()
            .and_then(|value| value.strip_prefix("${"))
            .and_then(|value| value.strip_suffix('}'))
            .expect("a reference");
        assert_eq!(
            attachment.templates.get(variable).map(SecretValue::expose),
            Some("${APPWRITE_API_KEY}")
        );
        assert_eq!(document["mcpServers"]["remote"]["type"], "http");

        let name = file
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a file name");
        assert!(
            name.starts_with("mcp-") && name.ends_with(".json"),
            "{name}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file.path())
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn the_file_is_deleted_when_its_handle_drops() {
        let attachment = rendered(&McpConfig::default().with_server("appwrite", appwrite()));
        let path = attachment.file.path().to_path_buf();
        assert!(path.exists());

        drop(attachment);
        assert!(!path.exists());
    }

    #[test]
    fn nothing_is_rendered_without_an_attachable_server() {
        assert!(
            McpConfig::default()
                .render(AgentKind::Claude)
                .expect("rendered")
                .is_none()
        );
        assert!(
            McpConfig::default()
                .with_server("broken", broken())
                .render(AgentKind::Claude)
                .expect("rendered")
                .is_none()
        );
    }

    #[test]
    fn an_invalid_server_is_skipped_rather_than_sinking_the_rest() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server("broken", broken());

        let document = read(&rendered(&config).file);
        let servers = document["mcpServers"].as_object().expect("servers");
        assert!(servers.contains_key("appwrite"));
        assert!(!servers.contains_key("broken"));

        assert_eq!(config.allowed_tools(), ["mcp__appwrite"]);
        assert!(config.redacted()["mcpServers"].get("broken").is_none());
    }

    /// A CLI's own rule for a remote server's references is all that keeps a
    /// credential from a server a configuration names, so nothing a remote
    /// server refers to may reach the child by any other road: neither
    /// expanded into a generated variable nor handed over from the host
    /// under its own name.
    #[test]
    fn the_child_is_never_handed_a_credential_a_remote_server_refers_to() {
        let settings = CliSettings::default()
            .with_credential(Credential::key("ANTHROPIC_API_KEY", "sk-ant-explicit"))
            .with_mcp_server(
                "remote",
                McpServer::remote("https://mcp.example.com/${NPM_TOKEN}")
                    .with_header("Authorization", "Bearer ${ANTHROPIC_API_KEY}")
                    .with_header("X-Npm", "token ${NPM_TOKEN}"),
            );
        let attachment = settings
            .mcp
            .render(AgentKind::Claude)
            .expect("rendered")
            .expect("an attachment");
        let host = |variable: &str| match variable {
            "PATH" => Some(OsString::from("/usr/bin:/bin")),
            "ANTHROPIC_API_KEY" => Some(OsString::from("sk-ant-host-key")),
            "NPM_TOKEN" => Some(OsString::from("npm-host-secret")),
            _ => None,
        };

        let environment = Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host);
        let mut command = Command::new("true");
        environment.apply(&mut command);

        for (variable, value) in command.as_std().get_envs() {
            let value = value.map(|value| value.to_string_lossy().into_owned());
            let value = value.unwrap_or_default();
            assert!(
                !value.contains("sk-ant-host-key") && !value.contains("npm-host-secret"),
                "the child was handed a credential as {variable:?}={value}"
            );
        }
    }

    #[test]
    fn a_rendered_file_holds_no_literal_secret_and_the_attachment_carries_it() {
        let config = McpConfig::default().with_server(
            "grafana",
            McpServer {
                environment: [
                    (
                        "GRAFANA_TOKEN".to_string(),
                        SecretValue::new("glsa_realsecret"),
                    ),
                    (
                        "GRAFANA_URL".to_string(),
                        SecretValue::new("${GRAFANA_URL}"),
                    ),
                ]
                .into(),
                ..appwrite()
            },
        );

        let attachment = rendered(&config);

        let contents = std::fs::read_to_string(attachment.file.path()).expect("the file");
        assert!(!contents.contains("glsa_realsecret"), "{contents}");
        let document: Value = serde_json::from_str(&contents).expect("JSON");
        let environment = &document["mcpServers"]["grafana"]["env"];
        let variable = environment["GRAFANA_TOKEN"]
            .as_str()
            .and_then(|value| value.strip_prefix("${"))
            .and_then(|value| value.strip_suffix('}'))
            .expect("a reference");
        assert_eq!(
            attachment
                .environment
                .get(variable)
                .map(SecretValue::expose),
            Some("glsa_realsecret")
        );
        assert_eq!(
            attachment
                .templates
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["${GRAFANA_URL}"]
        );
        assert!(!contents.contains("GRAFANA_URL}"), "{contents}");
        assert!(format!("{attachment:?}").contains("[REDACTED]"));
        assert!(!format!("{attachment:?}").contains("glsa_realsecret"));
    }

    /// A default is part of the configuration, so a secret written as one
    /// must stay out of the file like any other literal.
    #[test]
    fn a_stdio_servers_default_never_reaches_the_file() {
        let config = McpConfig::default().with_server(
            "notes",
            McpServer::command("notes-server", ["--key=${NOTES_KEY:-default-literal-key}"])
                .with_environment("NOTES_TOKEN", "${NOTES_TOKEN:-default-literal-token}"),
        );

        let attachment = rendered(&config);

        let contents = std::fs::read_to_string(attachment.file.path()).expect("the file");
        assert!(!contents.contains("default-literal"), "{contents}");
    }

    #[test]
    fn allowed_tools_cover_every_server_scoped_or_not() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server(
                "grafana",
                McpServer {
                    tools: vec!["list_datasources".to_string()],
                    ..appwrite()
                },
            );

        assert_eq!(
            config.allowed_tools(),
            ["mcp__appwrite", "mcp__grafana__list_datasources"]
        );
    }

    #[test]
    fn scoped_tools_leave_out_every_server_that_names_none() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server(
                "grafana",
                McpServer {
                    tools: vec!["list_datasources".to_string()],
                    ..appwrite()
                },
            );

        assert_eq!(config.scoped_tools(), ["mcp__grafana__list_datasources"]);
    }

    #[test]
    fn the_log_view_keeps_the_document_shape_without_its_secrets() {
        let config = McpConfig::default().with_server(
            "grafana",
            McpServer {
                environment: [(
                    "GRAFANA_TOKEN_LITERAL".to_string(),
                    SecretValue::new("glsa_realsecret"),
                )]
                .into(),
                ..appwrite()
            },
        );

        let view = config.redacted();
        assert_eq!(view["mcpServers"]["grafana"]["command"], "uvx");
        assert!(!view.to_string().contains("glsa_realsecret"));
    }

    #[test]
    fn a_server_or_tool_name_that_could_widen_the_allowed_tools_never_attaches() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server("grafana Bash", appwrite())
            .with_server("bash,Edit", appwrite())
            .with_server(
                "remote",
                McpServer {
                    tools: vec!["query Bash".to_string()],
                    ..appwrite()
                },
            )
            .with_server(
                "scoped",
                McpServer {
                    tools: vec!["query".to_string(), "list,Edit".to_string()],
                    ..appwrite()
                },
            );

        assert_eq!(config.allowed_tools(), ["mcp__appwrite"]);
        let document = read(&rendered(&config).file);
        let servers: Vec<&String> = document["mcpServers"]
            .as_object()
            .expect("servers")
            .keys()
            .collect();
        assert_eq!(servers, ["appwrite"]);
    }

    #[test]
    fn a_configuration_reads_from_the_clis_own_document_and_writes_it_back() {
        let wrapped: McpConfig = serde_json::from_value(serde_json::json!({
            "mcpServers": {
                "appwrite": {"command": "uvx", "arguments": ["mcp-server-appwrite"], "environment": {"KEY": "${KEY}"}}
            }
        }))
        .expect("a configuration");

        let server = &wrapped.servers["appwrite"];
        assert_eq!(server.arguments, ["mcp-server-appwrite"]);
        assert_eq!(
            server.environment.get("KEY").map(SecretValue::expose),
            Some("${KEY}")
        );

        let written = serde_json::to_value(&wrapped).expect("serialisable");
        assert_eq!(
            written["mcpServers"]["appwrite"]["args"][0],
            "mcp-server-appwrite"
        );
        assert_eq!(
            serde_json::from_value::<McpConfig>(written).expect("a round trip"),
            wrapped
        );
    }

    #[test]
    fn a_malformed_server_is_skipped_and_a_document_of_the_wrong_shape_is_an_error() {
        let (config, logs) = captured_logs(|| {
            serde_json::from_value::<McpConfig>(serde_json::json!({
                "mcpServers": {
                    "appwrite": {"command": "uvx", "args": "not a list"},
                    "notes": {"command": "notes-server"}
                }
            }))
        });

        let config = config.expect("the servers that could be read");
        assert_eq!(config.servers.keys().collect::<Vec<_>>(), ["notes"]);
        assert!(
            logs.contains("invalid MCP server 'appwrite': `args` must be a list of strings"),
            "{logs}"
        );
        for document in [
            serde_json::json!({"mcpServers": "not an object"}),
            serde_json::json!([1, 2]),
            serde_json::json!("notes"),
        ] {
            assert!(
                serde_json::from_value::<McpConfig>(document.clone()).is_err(),
                "{document}"
            );
        }
    }

    /// One entry this crate cannot read must not take the others down with
    /// it, and an entry written for another client, or copied from a
    /// server's own documentation, reads as that client reads it.
    #[test]
    fn a_server_another_client_describes_loads_next_to_the_rest() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "notes": {"command": "notes-server", "args": ["mcp"]},
                    "docs": {
                        "type": "streamable-http",
                        "url": "https://docs.example.com/mcp",
                        "tools": [{"name": "search", "description": "Search the docs"}, "fetch"]
                    }
                }
            }"#,
        )
        .expect("a configuration");

        let notes = &config.servers["notes"];
        assert!(config.enabled().any(|(name, _)| name == "notes"));
        assert!(notes.valid(), "{notes:?}");
        assert_eq!(notes.command.as_deref(), Some("notes-server"));
        let docs = &config.servers["docs"];
        assert_eq!(docs.transport, Some(McpTransport::Http));
        assert_eq!(docs.tools, ["search", "fetch"]);
        assert!(docs.valid(), "{docs:?}");
    }

    #[test]
    fn a_server_name_holding_the_separator_never_attaches() {
        let config = McpConfig::default()
            .with_server("my", appwrite())
            .with_server("my__server", appwrite());

        assert_eq!(config.allowed_tools(), ["mcp__my"]);
        assert_eq!(config.attachable().count(), 1);
    }

    #[test]
    fn a_configuration_reads_as_a_plain_map_of_servers() {
        let config: McpConfig = serde_json::from_value(serde_json::json!({
            "appwrite": {"command": "uvx", "args": ["mcp-server-appwrite"]},
            "remote": {"url": "https://example.com/mcp", "type": "sse"}
        }))
        .expect("a configuration");

        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers["remote"].transport, Some(McpTransport::Sse));
        assert!(!config.is_empty());
        assert!(McpConfig::default().is_empty());
    }

    #[test]
    fn a_disabled_server_is_absent_from_the_file_and_from_the_allowed_tools() {
        let config = McpConfig::default()
            .with_server("appwrite", appwrite())
            .with_server(
                "grafana",
                McpServer {
                    tools: vec!["list_datasources".to_string()],
                    ..appwrite()
                }
                .disable(),
            );

        let document = read(&rendered(&config).file);
        let servers: Vec<&String> = document["mcpServers"]
            .as_object()
            .expect("servers")
            .keys()
            .collect();
        assert_eq!(servers, ["appwrite"]);
        assert_eq!(config.allowed_tools(), ["mcp__appwrite"]);
        assert!(config.scoped_tools().is_empty());
        assert!(config.redacted()["mcpServers"].get("grafana").is_none());
        assert_eq!(config.attachable().count(), 1);
        assert!(config.servers.contains_key("grafana"), "kept as configured");
    }

    #[test]
    fn a_configuration_whose_every_server_is_disabled_is_empty_and_renders_nothing() {
        let config = McpConfig::default().with_server("appwrite", appwrite().disable());

        assert!(config.is_empty());
        assert!(
            config
                .render(AgentKind::Claude)
                .expect("rendered")
                .is_none()
        );
        assert!(config.allowed_tools().is_empty());
    }

    #[test]
    fn the_rendered_file_never_carries_inherit_environment() {
        let config = McpConfig::default().with_server("appwrite", appwrite().inherit_environment());

        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let attachment = config
                .render(agent)
                .expect("rendered")
                .expect("an attachment");
            let contents = std::fs::read_to_string(attachment.file.path()).expect("the file");
            assert!(!contents.contains("inherit_environment"), "{contents}");
            assert!(!contents.contains("disabled"), "{contents}");
        }
    }

    #[test]
    fn the_rendered_file_carries_a_working_directory_only_for_an_agent_that_documents_it() {
        let config = McpConfig::default().with_server(
            "appwrite",
            appwrite().with_working_directory("/srv/appwrite"),
        );

        let claude = read(&rendered(&config).file);
        let codex = read(
            &config
                .render(AgentKind::Codex)
                .expect("rendered")
                .expect("an attachment")
                .file,
        );

        assert!(
            claude["mcpServers"]["appwrite"].get("cwd").is_none(),
            "{claude}"
        );
        assert_eq!(codex["mcpServers"]["appwrite"]["cwd"], "/srv/appwrite");
    }

    #[test]
    fn parses_cursor_mcp_servers_document() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "notes": {
                        "command": "notes-server",
                        "args": ["mcp"]
                    },
                    "docs": {
                        "command": "uvx",
                        "args": ["mcp-server-fetch"],
                        "env": {"FOO": "bar"},
                        "cwd": "/srv/docs"
                    }
                }
            }"#,
        )
        .expect("a configuration");

        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers["notes"].arguments, ["mcp"]);
        let docs = &config.servers["docs"];
        assert_eq!(
            docs.environment.get("FOO").map(SecretValue::expose),
            Some("bar")
        );
        assert_eq!(docs.working_directory, Some(PathBuf::from("/srv/docs")));
        assert!(config.auto_connect);
    }

    #[test]
    fn parses_bare_server_map() {
        let config = McpConfig::from_json_str(
            r#"{ "notes": { "command": "/opt/homebrew/bin/notes-server", "args": ["mcp"] } }"#,
        )
        .expect("a configuration");

        assert_eq!(config.servers.len(), 1);
        assert_eq!(
            config.servers["notes"].command.as_deref(),
            Some("/opt/homebrew/bin/notes-server")
        );
    }

    #[test]
    fn parses_a_servers_document() {
        let config = McpConfig::from_json_str(
            r#"{ "servers": { "notes": { "command": "notes-server" } } }"#,
        )
        .expect("a configuration");

        assert_eq!(
            config.servers.keys().collect::<Vec<_>>(),
            ["notes"],
            "servers is the wrapper, not a server"
        );
    }

    #[test]
    fn a_bare_map_may_still_name_a_server_servers() {
        let config = McpConfig::from_json_str(
            r#"{ "servers": { "command": "notes-server", "args": ["mcp"] } }"#,
        )
        .expect("a configuration");

        assert_eq!(
            config.servers["servers"].command.as_deref(),
            Some("notes-server")
        );
    }

    #[test]
    fn keeps_disabled_and_remote_servers_for_each_launcher_to_decide() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "off": { "command": "x", "disabled": true },
                    "remote": { "url": "http://localhost:3000/mcp" },
                    "ok": { "command": "notes-server", "args": ["mcp"] }
                }
            }"#,
        )
        .expect("a configuration");

        assert_eq!(config.servers.len(), 3);
        assert!(config.servers["off"].disabled);
        assert_eq!(
            config.enabled().map(|(name, _)| name).collect::<Vec<_>>(),
            ["ok", "remote"]
        );
    }

    #[test]
    fn from_file_round_trips() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "notes": { "command": "notes-server", "args": ["mcp"] } } }"#,
        )
        .expect("written");

        let config = McpConfig::from_file(&path).expect("a configuration");

        assert!(config.servers.contains_key("notes"));
        assert!(matches!(
            McpConfig::from_file(&directory.path().join("missing.json")),
            Err(McpConfigError::Io { .. })
        ));
    }

    #[test]
    fn rejects_non_object() {
        let error = McpConfig::from_json_str("[1, 2]").expect_err("not a document");
        assert!(error.to_string().contains("expected mcpServers"), "{error}");

        let error =
            McpConfig::from_json_str(r#"{ "mcpServers": [] }"#).expect_err("not a document");
        assert!(error.to_string().contains("must be an object"), "{error}");

        let error = McpConfig::from_json_str("{").expect_err("not JSON");
        assert!(matches!(error, McpConfigError::Json(_)), "{error}");
    }

    #[test]
    fn a_malformed_server_is_named_in_the_warning_that_skips_it() {
        let (config, logs) = captured_logs(|| {
            McpConfig::from_json_str(
                r#"{ "mcpServers": { "notes": { "command": "x", "args": "mcp" } } }"#,
            )
        });

        assert!(config.expect("a configuration").servers.is_empty());
        assert!(logs.contains("invalid MCP server 'notes'"), "{logs}");
    }

    #[test]
    fn a_transport_this_crate_cannot_attach_is_read_but_never_attached() {
        let config = McpConfig::from_json_str(
            r#"{ "mcpServers": {
                "socket": { "type": "ws", "url": "wss://mcp.example.com" },
                "notes": { "command": "notes-server" }
            } }"#,
        )
        .expect("a configuration");

        assert_eq!(
            config.servers["socket"].transport,
            Some(McpTransport::Unsupported)
        );
        let (attachment, logs) = captured_logs(|| config.render(AgentKind::Claude));
        let document = read(&attachment.expect("rendered").expect("an attachment").file);
        assert!(document["mcpServers"].get("socket").is_none(), "{document}");
        assert!(document["mcpServers"].get("notes").is_some(), "{document}");
        assert!(logs.contains("socket"), "{logs}");
    }

    fn shell() -> McpServer {
        McpServer::command("sh", Vec::<String>::new())
    }

    #[test]
    fn fallback_skips_when_servers_already_configured() {
        let config =
            McpConfig::from_json_str(r#"{ "mcpServers": { "docs": { "command": "echo" } } }"#)
                .expect("a configuration")
                .fallback("shell", shell());

        assert_eq!(config.servers.keys().collect::<Vec<_>>(), ["docs"]);
    }

    #[test]
    fn fallback_attaches_a_command_on_path_when_nothing_is_configured() {
        let config = McpConfig::default().fallback("shell", shell());
        assert_eq!(
            config.servers,
            BTreeMap::from([("shell".to_string(), shell())])
        );
    }

    #[test]
    fn fallback_attaches_when_every_configured_server_is_disabled() {
        let config = McpConfig::default()
            .with_server("docs", shell().disable())
            .fallback("shell", shell());

        assert_eq!(
            config.enabled().map(|(name, _)| name).collect::<Vec<_>>(),
            ["shell"]
        );
    }

    #[test]
    fn fallback_skips_a_command_that_is_not_on_path() {
        let config = McpConfig::default().fallback(
            "missing",
            McpServer::command("definitely-not-a-real-mcp-server-xyz", Vec::<String>::new()),
        );
        assert!(config.servers.is_empty());
    }

    #[test]
    fn fallback_attaches_a_remote_server_and_never_an_invalid_one() {
        let remote =
            McpConfig::default().fallback("docs", McpServer::remote("https://example.com/mcp"));
        let invalid = McpConfig::default().fallback("broken", broken());

        assert!(remote.servers.contains_key("docs"));
        assert!(invalid.servers.is_empty());
    }

    #[test]
    fn fallback_skips_when_auto_connect_is_off() {
        let config = McpConfig {
            auto_connect: false,
            ..McpConfig::default()
        }
        .fallback("shell", shell());
        assert!(config.servers.is_empty());
    }

    const MARKER: &str = "marker-7Q2-secret";

    /// A document with a secret pasted into a field of the wrong type.
    fn misplaced() -> String {
        format!(
            r#"{{"mcpServers": {{"remote": {{"url": "https://mcp.example.com/mcp", "headers": "Bearer {MARKER}"}}}}}}"#
        )
    }

    /// A value of the wrong type can still be a secret pasted into the wrong
    /// field, and an error reading a configuration reaches a log.
    #[test]
    fn an_unreadable_server_never_quotes_the_document() {
        let document = misplaced();
        let read = environment(&[(variable("ACME", SERVERS_VARIABLE), document.as_str())]);

        let (loaded, logs) = captured_logs(|| McpConfig::load("ACME", &read, None));
        let parsed = McpConfig::from_json_str(&document);

        assert!(!loaded.servers.contains_key("remote"));
        assert!(logs.contains("remote"), "{logs}");
        assert!(!logs.contains(MARKER), "{logs}");
        if let Err(error) = &parsed {
            let mut shown = vec![error.to_string(), format!("{error:?}")];
            let mut source = error.source();
            while let Some(cause) = source {
                shown.push(cause.to_string());
                shown.push(format!("{cause:?}"));
                source = cause.source();
            }
            for text in shown {
                assert!(!text.contains(MARKER), "{text}");
            }
        }
    }

    #[test]
    fn an_unreadable_server_names_the_field_and_the_type_it_must_hold() {
        let document: Value = serde_json::from_str(&misplaced()).expect("JSON");
        let error = McpConfig::server("remote", &document["mcpServers"]["remote"])
            .expect_err("an unreadable server");

        assert!(
            matches!(
                &error,
                McpConfigError::Server {
                    name,
                    field: Some("headers"),
                    expected: "an object of strings",
                } if name == "remote"
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "invalid MCP server 'remote': `headers` must be an object of strings"
        );
        assert!(error.source().is_none());
    }

    #[test]
    fn a_prefix_names_every_variable_and_the_default_file() {
        assert_eq!(variable("ACME", ENABLED_VARIABLE), "ACME_MCP_ENABLED");
        assert_eq!(variable("ACME", SERVERS_VARIABLE), "ACME_MCP_SERVERS");
        assert_eq!(variable("ACME", CONFIG_VARIABLE), "ACME_MCP_CONFIG");
        assert_eq!(
            variable("ACME", AUTO_CONNECT_VARIABLE),
            "ACME_MCP_AUTO_CONNECT"
        );
        assert_eq!(
            default_path(Path::new("/home/user"), "ACME"),
            PathBuf::from("/home/user/.acme/mcp.json")
        );
        assert_eq!(
            default_path(Path::new("/home/user"), DEFAULT_PREFIX),
            PathBuf::from("/home/user/.abnegate/mcp.json")
        );
    }

    fn environment(variables: &[(String, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let variables: HashMap<String, String> = variables
            .iter()
            .map(|(name, value)| (name.clone(), value.to_string()))
            .collect();
        move |name| variables.get(name).cloned()
    }

    const DOCS: &str = r#"{ "mcpServers": { "docs": { "command": "echo" } } }"#;

    /// The prefix is how an application keeps its configuration apart from
    /// every other application's, so nothing may be read from outside it.
    #[test]
    fn from_environment_reads_only_variables_under_its_own_prefix() {
        let read = environment(&[
            (variable("ACME", SERVERS_VARIABLE), DOCS),
            (variable("ACME", AUTO_CONNECT_VARIABLE), "off"),
        ]);

        let config = McpConfig::load("ACME", &read, None);
        let other = McpConfig::load("OTHER", &read, None);

        assert_eq!(config.servers.keys().collect::<Vec<_>>(), ["docs"]);
        assert!(!config.auto_connect);
        assert!(other.is_empty(), "another prefix read this one's servers");
        assert!(other.auto_connect);
    }

    #[test]
    fn a_disabled_prefix_attaches_nothing_not_even_a_fallback() {
        let read = environment(&[
            (variable("ACME", ENABLED_VARIABLE), "0"),
            (variable("ACME", SERVERS_VARIABLE), DOCS),
        ]);

        let config = McpConfig::load("ACME", read, None).fallback("shell", shell());

        assert!(config.servers.is_empty());
        assert!(!config.auto_connect);
    }

    #[test]
    fn inline_servers_win_over_a_config_file() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "file": { "command": "echo" } } }"#,
        )
        .expect("written");
        let path = path.to_string_lossy().into_owned();

        let inline = McpConfig::load(
            "ACME",
            environment(&[
                (variable("ACME", SERVERS_VARIABLE), DOCS),
                (variable("ACME", CONFIG_VARIABLE), &path),
            ]),
            None,
        );
        let file = McpConfig::load(
            "ACME",
            environment(&[(variable("ACME", CONFIG_VARIABLE), &path)]),
            None,
        );

        assert_eq!(inline.servers.keys().collect::<Vec<_>>(), ["docs"]);
        assert_eq!(file.servers.keys().collect::<Vec<_>>(), ["file"]);
    }

    #[test]
    fn the_default_file_is_read_from_the_prefix_directory_under_home() {
        let home = tempfile::tempdir().expect("a directory");
        let path = default_path(home.path(), "ACME");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("created");
        std::fs::write(&path, DOCS).expect("written");

        let config = McpConfig::load("ACME", environment(&[]), Some(home.path().to_path_buf()));
        let other = McpConfig::load("OTHER", environment(&[]), Some(home.path().to_path_buf()));

        assert_eq!(config.servers.keys().collect::<Vec<_>>(), ["docs"]);
        assert!(other.servers.is_empty());
    }

    #[test]
    fn invalid_inline_servers_leave_nothing_configured() {
        let config = McpConfig::load(
            "ACME",
            environment(&[(variable("ACME", SERVERS_VARIABLE), "[1, 2]")]),
            None,
        );
        assert!(config.servers.is_empty());
        assert!(config.auto_connect);
    }

    #[test]
    fn parse_flag_keeps_default_for_empty_and_unknown() {
        assert!(parse_flag("", true));
        assert!(!parse_flag("", false));
        assert!(parse_flag("maybe", true));
        assert!(!parse_flag("maybe", false));
        assert!(parse_flag(" true ", true));
        assert!(!parse_flag("0", true));
        assert!(!parse_flag("off", true));
        assert!(!parse_flag("false", true));
        assert!(parse_flag("1", false));
        assert!(parse_flag("yes", false));
        assert!(parse_flag("on", false));
    }

    #[test]
    fn auto_connect_is_on_by_default_and_never_written() {
        assert!(McpConfig::default().auto_connect);

        let written = serde_json::to_value(McpConfig {
            auto_connect: false,
            ..McpConfig::default().with_server("appwrite", appwrite())
        })
        .expect("serialisable");

        assert_eq!(
            written
                .as_object()
                .expect("an object")
                .keys()
                .collect::<Vec<_>>(),
            ["mcpServers"]
        );
    }
}
