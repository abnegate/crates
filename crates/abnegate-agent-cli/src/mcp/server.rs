use std::collections::BTreeMap;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::mcp::transport::McpTransport;

const PREFIX: &str = "mcp__";
const SEPARATOR: &str = "__";

/// One MCP server, as a CLI's MCP configuration describes it.
///
/// Exactly one of `command` and `url` must be set. Environment and header
/// values are held as secrets, so a literal key never reaches a log line
/// through `Debug`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    /// The command that starts a stdio server, such as `uvx` or `npx`.
    pub command: Option<String>,
    #[serde(rename = "args")]
    pub arguments: Vec<String>,
    #[serde(rename = "env")]
    pub environment: BTreeMap<String, SecretValue>,
    /// Where an HTTP or SSE server listens.
    pub url: Option<String>,
    #[serde(rename = "type")]
    pub transport: Option<McpTransport>,
    pub headers: BTreeMap<String, SecretValue>,
    /// The tools to allow without prompting. Empty allows every tool the
    /// server offers.
    pub tools: Vec<String>,
}

impl McpServer {
    /// Whether exactly one of `command` and `url` is set, and any explicit
    /// transport agrees with it.
    pub fn valid(&self) -> bool {
        match (&self.command, &self.url, self.transport) {
            (Some(_), None, transport) => transport.is_none_or(|transport| !transport.remote()),
            (None, Some(_), transport) => transport.is_none_or(McpTransport::remote),
            _ => false,
        }
    }

    /// The `--allowedTools` entries for this server under `name`.
    pub fn allowed_tools(&self, name: &str) -> Vec<String> {
        if self.tools.is_empty() {
            return vec![format!("{PREFIX}{name}")];
        }
        self.tools
            .iter()
            .map(|tool| format!("{PREFIX}{name}{SEPARATOR}{tool}"))
            .collect()
    }

    /// This server as the CLI's configuration file holds it, with every
    /// secret exposed.
    pub(crate) fn entry(&self) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(self.arguments));
            if !self.environment.is_empty() {
                entry.insert("env".to_string(), exposed(&self.environment));
            }
            if let Some(transport) = self.transport {
                entry.insert("type".to_string(), json!(transport));
            }
        } else if let Some(url) = &self.url {
            let transport = self.transport.unwrap_or(McpTransport::Http);
            entry.insert("type".to_string(), json!(transport));
            entry.insert("url".to_string(), json!(url));
            if !self.headers.is_empty() {
                entry.insert("headers".to_string(), exposed(&self.headers));
            }
        }
        Value::Object(entry)
    }

    /// This server for a log line: structure intact, and every environment
    /// or header value masked unless it is nothing but a `${VAR}` reference,
    /// which names a secret without holding one.
    pub(crate) fn redacted(&self) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(self.arguments));
            if !self.environment.is_empty() {
                entry.insert("env".to_string(), masked(&self.environment));
            }
        } else if let Some(url) = &self.url {
            entry.insert("url".to_string(), json!(url));
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

fn exposed(values: &BTreeMap<String, SecretValue>) -> Value {
    values
        .iter()
        .map(|(key, value)| (key.clone(), json!(value.expose())))
        .collect::<Map<String, Value>>()
        .into()
}

fn masked(values: &BTreeMap<String, SecretValue>) -> Value {
    values
        .iter()
        .map(|(key, value)| {
            let value = value.expose();
            let shown = if reference(value) { value } else { REDACTED };
            (key.clone(), json!(shown))
        })
        .collect::<Map<String, Value>>()
        .into()
}

fn reference(value: &str) -> bool {
    value
        .trim()
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| !name.contains("${") && !name.contains('}'))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use abnegate_secret::SecretValue;
    use serde_json::json;

    use super::McpServer;
    use super::reference;
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
    fn a_stdio_entry_exposes_its_environment_for_the_cli_to_expand() {
        let server = McpServer {
            environment: secrets(&[("APPWRITE_API_KEY", "${APPWRITE_API_KEY}")]),
            ..stdio()
        };

        let entry = server.entry();
        assert_eq!(entry["command"], "uvx");
        assert_eq!(entry["args"][0], "mcp-server-appwrite");
        assert_eq!(entry["env"]["APPWRITE_API_KEY"], "${APPWRITE_API_KEY}");
        assert!(entry.get("type").is_none());
        assert!(entry.get("url").is_none());
    }

    #[test]
    fn a_json_blob_in_the_environment_is_written_verbatim() {
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

        let entry = server.entry();
        assert_eq!(
            entry["env"]["GRAFANA_SERVICE_ACCOUNT_TOKEN"],
            "${GRAFANA_SERVICE_ACCOUNT_TOKEN}"
        );
        assert_eq!(entry["env"]["GRAFANA_EXTRA_HEADERS"], headers);
    }

    #[test]
    fn an_http_entry_carries_only_http_fields() {
        let server = McpServer {
            transport: Some(McpTransport::Http),
            headers: secrets(&[("Authorization", "Bearer ${TOKEN}")]),
            ..http()
        };

        let entry = server.entry();
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        assert_eq!(entry["headers"]["Authorization"], "Bearer ${TOKEN}");
        assert!(entry.get("command").is_none());
        assert!(entry.get("args").is_none());
        assert!(entry.get("env").is_none());
    }

    #[test]
    fn a_url_without_a_type_defaults_to_http_and_sse_is_kept() {
        assert_eq!(http().entry()["type"], "http");
        assert_eq!(
            McpServer {
                transport: Some(McpTransport::Sse),
                ..http()
            }
            .entry()["type"],
            "sse"
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
    fn only_a_single_whole_reference_counts_as_one() {
        assert!(reference("${TOKEN}"));
        assert!(reference("  ${TOKEN}  "));
        assert!(!reference("Bearer ${TOKEN}"));
        assert!(!reference("${A}${B}"));
        assert!(!reference("${A}-${B}"));
        assert!(!reference("literal"));
        assert!(!reference("${"));
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
}
