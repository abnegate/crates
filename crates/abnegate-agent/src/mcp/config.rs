use serde_json::Value;
use std::path::{Path, PathBuf};

use super::{McpConfigError, McpServerSpec};

/// The prefix [`McpConfig::from_env`] reads its variables under.
pub const DEFAULT_PREFIX: &str = "ABNEGATE";

const ENABLED: &str = "ENABLED";
const SERVERS: &str = "SERVERS";
const CONFIG: &str = "CONFIG";
const AUTO_CONNECT: &str = "AUTO_CONNECT";
const CONFIG_FILE: &str = "mcp.json";

/// The MCP servers to attach for one agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub servers: Vec<McpServerSpec>,
    /// Whether [`fallback`](Self::fallback) may attach a server when none is
    /// configured.
    pub auto_connect: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            auto_connect: true,
        }
    }
}

impl McpConfig {
    /// Load from the environment under [`DEFAULT_PREFIX`].
    pub fn from_env() -> Self {
        Self::with_prefix(DEFAULT_PREFIX)
    }

    /// Load from the environment under an application's own prefix: `ACME`
    /// reads `ACME_MCP_SERVERS`, `ACME_MCP_CONFIG` and `~/.acme/mcp.json`.
    ///
    /// Never fails: a configuration that cannot be read is logged and skipped,
    /// and what is left is an empty set of servers.
    pub fn with_prefix(prefix: &str) -> Self {
        Self::load(prefix, |name| std::env::var(name).ok(), dirs::home_dir())
    }

    /// [`with_prefix`](Self::with_prefix), reading variables through `read` and
    /// the default file from beneath `home`.
    fn load(prefix: &str, read: impl Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Self {
        let flag =
            |name: &str| read(&variable(prefix, name)).is_none_or(|value| parse_flag(&value, true));
        if !flag(ENABLED) {
            return Self {
                servers: Vec::new(),
                auto_connect: false,
            };
        }
        Self {
            auto_connect: flag(AUTO_CONNECT),
            ..Self::configured(prefix, &read, home)
        }
    }

    fn configured(
        prefix: &str,
        read: &impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Self {
        let servers = variable(prefix, SERVERS);
        if let Some(raw) = read(&servers).filter(|raw| !raw.trim().is_empty()) {
            return Self::from_json_str(&raw).unwrap_or_else(|error| {
                tracing::warn!(variable = %servers, error = %error, "MCP servers are invalid; ignoring");
                Self::default()
            });
        }

        let config = variable(prefix, CONFIG);
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

    /// Parse a Cursor-style `mcpServers` document or a bare name → spec map.
    pub fn from_json_str(raw: &str) -> Result<Self, McpConfigError> {
        let value: Value = serde_json::from_str(raw)?;
        Self::from_value(&value)
    }

    /// Read and parse a JSON config file.
    pub fn from_file(path: &Path) -> Result<Self, McpConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| McpConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&raw)
    }

    /// Parse `{ "mcpServers": { ... } }`, `{ "servers": { ... } }`, or a bare map.
    pub fn from_value(value: &Value) -> Result<Self, McpConfigError> {
        let map = if let Some(servers) = value.get("mcpServers").or_else(|| value.get("servers")) {
            servers
                .as_object()
                .ok_or_else(|| McpConfigError::Invalid("mcpServers must be an object".into()))?
        } else if let Some(object) = value
            .as_object()
            .filter(|object| object.values().all(Value::is_object))
        {
            object
        } else {
            return Err(McpConfigError::Invalid(
                "expected mcpServers object or a map of server specs".into(),
            ));
        };

        let mut servers = Vec::with_capacity(map.len());
        for (name, spec) in map {
            if spec.get("url").is_some() && spec.get("command").is_none() {
                tracing::warn!(
                    server = %name,
                    "skipping HTTP MCP server; only stdio servers are supported"
                );
                continue;
            }

            let mut parsed: McpServerSpec = serde_json::from_value(spec.clone())
                .map_err(|error| McpConfigError::Invalid(format!("server '{name}': {error}")))?;
            if parsed.command.trim().is_empty() {
                return Err(McpConfigError::Invalid(format!(
                    "server '{name}' is missing command"
                )));
            }
            parsed.name = name.clone();
            if !parsed.disabled {
                servers.push(parsed);
            }
        }

        Ok(Self {
            servers,
            ..Self::default()
        })
    }

    /// Attach `spec` when nothing else is configured, auto-connect is on, and
    /// its command is on `PATH`.
    pub fn fallback(mut self, spec: McpServerSpec) -> Self {
        if self.auto_connect && self.servers.is_empty() && command_on_path(&spec.command) {
            self.servers.push(spec);
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
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
    use super::*;
    use std::collections::HashMap;

    fn shell() -> McpServerSpec {
        McpServerSpec::new("shell", "sh", Vec::<String>::new())
    }

    #[test]
    fn parses_cursor_mcp_servers_document() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "magents": {
                        "command": "magents",
                        "args": ["mcp"]
                    },
                    "docs": {
                        "command": "uvx",
                        "args": ["mcp-server-fetch"],
                        "env": {"FOO": "bar"}
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(config.servers.len(), 2);
        let magents = config
            .servers
            .iter()
            .find(|server| server.name == "magents")
            .unwrap();
        assert_eq!(magents.args, ["mcp"]);
        let docs = config
            .servers
            .iter()
            .find(|server| server.name == "docs")
            .unwrap();
        assert_eq!(docs.env.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn parses_bare_server_map() {
        let config = McpConfig::from_json_str(
            r#"{ "magents": { "command": "/opt/homebrew/bin/magents", "args": ["mcp"] } }"#,
        )
        .unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].command, "/opt/homebrew/bin/magents");
    }

    #[test]
    fn skips_disabled_and_http_only_servers() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "off": { "command": "x", "disabled": true },
                    "remote": { "url": "http://localhost:3000/mcp" },
                    "ok": { "command": "magents", "args": ["mcp"] }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "ok");
    }

    #[test]
    fn from_file_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "magents": { "command": "magents", "args": ["mcp"] } } }"#,
        )
        .unwrap();
        let config = McpConfig::from_file(&path).unwrap();
        assert_eq!(config.servers[0].name, "magents");
    }

    #[test]
    fn rejects_non_object() {
        let error = McpConfig::from_json_str("[1, 2]").unwrap_err();
        assert!(error.to_string().contains("expected mcpServers"));
    }

    #[test]
    fn fallback_skips_when_servers_already_configured() {
        let config =
            McpConfig::from_json_str(r#"{ "mcpServers": { "docs": { "command": "echo" } } }"#)
                .unwrap()
                .fallback(shell());
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "docs");
    }

    #[test]
    fn fallback_attaches_a_command_on_path_when_nothing_is_configured() {
        let config = McpConfig::default().fallback(shell());
        assert_eq!(config.servers, vec![shell()]);
    }

    #[test]
    fn fallback_skips_a_command_that_is_not_on_path() {
        let config = McpConfig::default().fallback(McpServerSpec::new(
            "missing",
            "definitely-not-a-real-mcp-server-xyz",
            Vec::<String>::new(),
        ));
        assert!(config.is_empty());
    }

    #[test]
    fn fallback_skips_when_auto_connect_is_off() {
        let config = McpConfig {
            auto_connect: false,
            ..McpConfig::default()
        }
        .fallback(shell());
        assert!(config.is_empty());
    }

    #[test]
    fn a_prefix_names_every_variable_and_the_default_file() {
        assert_eq!(variable("ACME", ENABLED), "ACME_MCP_ENABLED");
        assert_eq!(variable("ACME", SERVERS), "ACME_MCP_SERVERS");
        assert_eq!(variable("ACME", CONFIG), "ACME_MCP_CONFIG");
        assert_eq!(variable("ACME", AUTO_CONNECT), "ACME_MCP_AUTO_CONNECT");
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
    fn with_prefix_reads_only_variables_under_its_own_prefix() {
        let read = environment(&[
            (variable("ACME", SERVERS), DOCS),
            (variable("ACME", AUTO_CONNECT), "off"),
        ]);

        let config = McpConfig::load("ACME", &read, None);
        let other = McpConfig::load("OTHER", &read, None);

        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "docs");
        assert!(!config.auto_connect);
        assert!(other.is_empty(), "another prefix read this one's servers");
        assert!(other.auto_connect);
    }

    #[test]
    fn a_disabled_prefix_attaches_nothing_not_even_a_fallback() {
        let read = environment(&[
            (variable("ACME", ENABLED), "0"),
            (variable("ACME", SERVERS), DOCS),
        ]);

        let config = McpConfig::load("ACME", read, None).fallback(shell());

        assert!(config.is_empty());
        assert!(!config.auto_connect);
    }

    #[test]
    fn inline_servers_win_over_a_config_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{ "mcpServers": { "file": { "command": "echo" } } }"#,
        )
        .unwrap();
        let path = path.to_string_lossy().into_owned();

        let inline = McpConfig::load(
            "ACME",
            environment(&[
                (variable("ACME", SERVERS), DOCS),
                (variable("ACME", CONFIG), &path),
            ]),
            None,
        );
        let file = McpConfig::load(
            "ACME",
            environment(&[(variable("ACME", CONFIG), &path)]),
            None,
        );

        assert_eq!(inline.servers[0].name, "docs");
        assert_eq!(file.servers[0].name, "file");
    }

    #[test]
    fn the_default_file_is_read_from_the_prefix_directory_under_home() {
        let home = tempfile::tempdir().unwrap();
        let path = default_path(home.path(), "ACME");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, DOCS).unwrap();

        let config = McpConfig::load("ACME", environment(&[]), Some(home.path().to_path_buf()));
        let other = McpConfig::load("OTHER", environment(&[]), Some(home.path().to_path_buf()));

        assert_eq!(config.servers[0].name, "docs");
        assert!(other.is_empty());
    }

    #[test]
    fn invalid_inline_servers_leave_nothing_configured() {
        let config = McpConfig::load(
            "ACME",
            environment(&[(variable("ACME", SERVERS), "[1, 2]")]),
            None,
        );
        assert!(config.is_empty());
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
}
