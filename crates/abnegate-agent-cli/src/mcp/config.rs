use std::collections::BTreeMap;
use std::io;
use std::io::Write;

use crate::mcp::attachment::McpAttachment;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::server::McpServer;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

const SERVERS: &str = "mcpServers";
const PREFIX: &str = "mcp-";
const SUFFIX: &str = ".json";

/// The MCP servers a run attaches, keyed by the name the agent knows each by.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpConfig {
    pub servers: BTreeMap<String, McpServer>,
}

impl McpConfig {
    pub fn with_server(mut self, name: impl Into<String>, server: McpServer) -> Self {
        self.servers.insert(name.into(), server);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// The servers that will actually attach: those with a
    /// [valid](McpServer::valid) transport, which a strict CLI would
    /// otherwise reject along with every other server.
    pub fn attachable(&self) -> impl Iterator<Item = (&str, &McpServer)> {
        self.servers
            .iter()
            .filter(|(_, server)| server.valid())
            .map(|(name, server)| (name.as_str(), server))
    }

    /// Write the attachable servers to a private temporary file for
    /// `--mcp-config`, or `None` when there are none, with what the child
    /// needs in its environment for the file to resolve.
    pub fn render(&self) -> io::Result<Option<McpAttachment>> {
        for (name, _) in self.servers.iter().filter(|(_, server)| !server.valid()) {
            tracing::warn!(
                server = %name,
                "skipping an MCP server: set exactly one of `command` and `url`, with a matching `type`"
            );
        }

        let mut placeholders = Placeholders::default();
        let servers: Map<String, Value> = self
            .attachable()
            .map(|(name, server)| (name.to_string(), server.entry(&mut placeholders)))
            .collect();
        if servers.is_empty() {
            return Ok(None);
        }

        let document = json!({ SERVERS: servers });
        let bytes = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
        let mut file = tempfile::Builder::new()
            .prefix(PREFIX)
            .suffix(SUFFIX)
            .tempfile()?;
        file.as_file_mut().write_all(&bytes)?;
        file.as_file_mut().flush()?;
        let Placeholders {
            environment,
            references,
        } = placeholders;
        Ok(Some(McpAttachment {
            file,
            environment,
            references,
        }))
    }

    /// What [`McpConfig::render`] writes, safe for a log line.
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

#[cfg(test)]
mod tests {
    use std::io::Read;

    use abnegate_secret::SecretValue;
    use serde_json::Value;
    use tempfile::NamedTempFile;

    use super::McpConfig;
    use crate::mcp::attachment::McpAttachment;
    use crate::mcp::server::McpServer;
    use crate::mcp::transport::McpTransport;

    fn rendered(config: &McpConfig) -> McpAttachment {
        config.render().expect("rendered").expect("an attachment")
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
        assert_eq!(appwrite["env"]["APPWRITE_API_KEY"], "${APPWRITE_API_KEY}");
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
        assert!(McpConfig::default().render().expect("rendered").is_none());
        assert!(
            McpConfig::default()
                .with_server("broken", broken())
                .render()
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
        assert_eq!(environment["GRAFANA_URL"], "${GRAFANA_URL}");
        assert!(attachment.references.contains("GRAFANA_URL"));
        assert!(format!("{attachment:?}").contains("[REDACTED]"));
        assert!(!format!("{attachment:?}").contains("glsa_realsecret"));
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
}
