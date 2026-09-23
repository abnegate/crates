use std::collections::BTreeMap;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::whole_reference;
use crate::mcp::transport::McpTransport;

const PREFIX: &str = "mcp__";
const SEPARATOR: &str = "__";
const NAME_PUNCTUATION: [char; 2] = ['_', '-'];

/// One MCP server, as a CLI's MCP configuration describes it.
///
/// Exactly one of `command` and `url` must be set. Environment and header
/// values are held as secrets, so a literal key never reaches a log line
/// through `Debug`, and never reaches the rendered configuration file
/// either: see [`McpAttachment`](crate::mcp::McpAttachment). A `${VAR}`
/// reference anywhere the CLI expands one is resolved from the host's
/// environment, which the child is given only the named variables of.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    /// The command that starts a stdio server, such as `uvx` or `npx`.
    pub command: Option<String>,
    #[serde(rename = "args", alias = "arguments")]
    pub arguments: Vec<String>,
    #[serde(rename = "env", alias = "environment")]
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

    /// Whether `name`, and every tool this server names, holds only
    /// letters, digits, `_` and `-`, and so is safe in `--allowedTools`, and
    /// `name` holds no `__`, which the CLI reads as the end of a server's
    /// name, so that one server's rule can never cover another's tools.
    pub fn nameable(&self, name: &str) -> bool {
        valid_name(name)
            && !name.contains(SEPARATOR)
            && self.tools.iter().all(|tool| valid_name(tool))
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

    /// This server as the CLI's configuration file holds it, with every
    /// literal environment or header value replaced by a reference to a
    /// variable in `placeholders`, and every variable it refers to noted
    /// there.
    pub(crate) fn entry(&self, placeholders: &mut Placeholders) -> Value {
        let mut entry = Map::new();
        if let Some(command) = &self.command {
            placeholders.note(command);
            for argument in &self.arguments {
                placeholders.note(argument);
            }
            entry.insert("command".to_string(), json!(command));
            entry.insert("args".to_string(), json!(self.arguments));
            if !self.environment.is_empty() {
                entry.insert(
                    "env".to_string(),
                    substituted(&self.environment, placeholders),
                );
            }
            if let Some(transport) = self.transport {
                entry.insert("type".to_string(), json!(transport));
            }
        } else if let Some(url) = &self.url {
            placeholders.note(url);
            let transport = self.transport.unwrap_or(McpTransport::Http);
            entry.insert("type".to_string(), json!(transport));
            entry.insert("url".to_string(), json!(url));
            if !self.headers.is_empty() {
                entry.insert(
                    "headers".to_string(),
                    substituted(&self.headers, placeholders),
                );
            }
        }
        Value::Object(entry)
    }

    /// This server for a log line: structure intact, every environment or
    /// header value masked unless it is nothing but a `${VAR}` reference,
    /// which names a secret without holding one, and anything credential
    /// shaped in the arguments or the URL redacted.
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

    use abnegate_secret::SecretValue;
    use serde_json::json;

    use super::McpServer;
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
    fn a_stdio_entry_exposes_its_environment_for_the_cli_to_expand() {
        let server = McpServer {
            environment: secrets(&[("APPWRITE_API_KEY", "${APPWRITE_API_KEY}")]),
            ..stdio()
        };

        let mut placeholders = Placeholders::default();
        let entry = server.entry(&mut placeholders);
        assert_eq!(entry["command"], "uvx");
        assert_eq!(entry["args"][0], "mcp-server-appwrite");
        assert_eq!(entry["env"]["APPWRITE_API_KEY"], "${APPWRITE_API_KEY}");
        assert!(entry.get("type").is_none());
        assert!(entry.get("url").is_none());
        assert!(placeholders.environment.is_empty());
        assert!(placeholders.references.contains("APPWRITE_API_KEY"));
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
        let mut placeholders = Placeholders::default();

        let entry = server.entry(&mut placeholders);

        assert_eq!(
            entry["env"]["GRAFANA_SERVICE_ACCOUNT_TOKEN"],
            "${GRAFANA_SERVICE_ACCOUNT_TOKEN}"
        );
        assert_eq!(entry["env"]["GRAFANA_EXTRA_HEADERS"], "${ABNEGATE_MCP_0}");
        assert_eq!(
            placeholders
                .templates
                .get("ABNEGATE_MCP_0")
                .map(SecretValue::expose),
            Some(headers)
        );
        assert!(placeholders.references.contains("CF_ACCESS_CLIENT_SECRET"));
    }

    #[test]
    fn an_http_entry_carries_only_http_fields() {
        let server = McpServer {
            transport: Some(McpTransport::Http),
            headers: secrets(&[("Authorization", "Bearer ${TOKEN}")]),
            ..http()
        };

        let mut placeholders = Placeholders::default();
        let entry = server.entry(&mut placeholders);
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://example.com/mcp");
        assert_eq!(entry["headers"]["Authorization"], "${ABNEGATE_MCP_0}");
        assert_eq!(
            placeholders
                .templates
                .get("ABNEGATE_MCP_0")
                .map(SecretValue::expose),
            Some("Bearer ${TOKEN}")
        );
        assert!(entry.get("command").is_none());
        assert!(entry.get("args").is_none());
        assert!(entry.get("env").is_none());
    }

    #[test]
    fn a_url_without_a_type_defaults_to_http_and_sse_is_kept() {
        assert_eq!(http().entry(&mut Placeholders::default())["type"], "http");
        assert_eq!(
            McpServer {
                transport: Some(McpTransport::Sse),
                ..http()
            }
            .entry(&mut Placeholders::default())["type"],
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
        let mut placeholders = Placeholders::default();

        let entries = [
            server.entry(&mut placeholders),
            remote.entry(&mut placeholders),
        ];

        for entry in &entries {
            let rendered = entry.to_string();
            assert!(!rendered.contains("glsa_realsecret"), "{rendered}");
            assert!(!rendered.contains("sk-live-secret"), "{rendered}");
        }
        assert_eq!(entries[0]["env"]["GRAFANA_TOKEN"], "${ABNEGATE_MCP_0}");
        assert_eq!(entries[1]["headers"]["Authorization"], "${ABNEGATE_MCP_1}");
        assert_eq!(
            placeholders
                .environment
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["glsa_realsecret", "Bearer sk-live-secret"]
        );
        assert_eq!(
            placeholders.references.iter().collect::<Vec<_>>(),
            ["GRAFANA_URL", "MCP_HOST"]
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
    fn a_log_view_redacts_credentials_in_arguments_and_urls() {
        let key = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let stdio = McpServer {
            arguments: vec!["--api-key".to_string(), key.to_string()],
            ..stdio()
        };
        let remote = McpServer {
            url: Some(format!("https://example.com/mcp?token={key}")),
            ..McpServer::default()
        };

        for view in [stdio.redacted(), remote.redacted()] {
            assert!(!view.to_string().contains("sk-ant-api03-AAAA"), "{view}");
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
}
