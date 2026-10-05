use std::collections::HashSet;

use abnegate_exec::Proxy;

use super::McpConfig;
use super::McpHub;
use crate::tool::ToolRegistry;

/// Attach every tool from a connected hub to `registry`, and return how many
/// were added.
///
/// Names are `server__tool` (`docs__search`). Collisions after sanitizing,
/// including with a tool already registered, are given a numeric suffix so
/// one tool cannot hide another. Only a server that contributed a tool is
/// recorded as attached, so a server with nothing to offer earns no prompt
/// guidance. The hub can be dropped afterwards: each tool holds its own
/// session handle.
pub fn register(registry: &mut ToolRegistry, hub: &McpHub) -> usize {
    let mut used: HashSet<String> = registry.names().into_iter().map(str::to_string).collect();
    let mut added = 0;
    for session in hub.sessions() {
        let tools = session.tools(&mut used);
        if tools.is_empty() {
            continue;
        }
        registry.attach(session.name.clone());
        added += tools.len();
        for tool in tools {
            registry.register(tool);
        }
    }
    added
}

/// The default file and command tools plus the tools of every server in
/// `config` that [`McpHub::connect`] launches.
///
/// A server that fails to start is logged and skipped.
pub async fn with_defaults_and_mcp(config: &McpConfig) -> ToolRegistry {
    with_defaults_and_mcp_through(config, &Proxy::from_environment()).await
}

/// [`with_defaults_and_mcp`], launching every server through `proxy` as
/// [`McpHub::connect_through`] does.
pub async fn with_defaults_and_mcp_through(config: &McpConfig, proxy: &Proxy) -> ToolRegistry {
    let mut registry = ToolRegistry::with_defaults();
    let hub = McpHub::connect_through(config, proxy).await;
    let added = register(&mut registry, &hub);
    if added > 0 {
        tracing::info!(
            tools = added,
            servers = hub.server_count(),
            "Attached MCP tools"
        );
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpServer;
    use crate::mcp::guidance;
    use crate::mcp::recorder::recorder;
    use crate::test_support::CONFIGURED_PROXY;
    use crate::test_support::assert_routed;

    #[test]
    fn register_on_empty_hub_is_noop() {
        let mut registry = ToolRegistry::new();
        let hub = McpHub::new();
        assert_eq!(register(&mut registry, &hub), 0);
        assert!(!registry.has_mcp());
        assert!(guidance(&registry, &[]).is_none());
    }

    #[tokio::test]
    async fn an_empty_config_yields_only_the_default_tools() {
        let registry = with_defaults_and_mcp(&McpConfig::default()).await;
        assert_eq!(
            registry.names().len(),
            ToolRegistry::with_defaults().names().len()
        );
        assert!(!registry.has_mcp());
    }

    /// A stdio server that records its environment to the path it is given,
    /// then answers the handshake, lists one tool, `echo`, and answers every
    /// call to it with `echoed`.
    fn echoing(path: &std::path::Path) -> McpServer {
        const SCRIPT: &str = r#"env > "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"echoing","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echoes.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed"}]}}\n' "$id" ;;
  esac
done"#;
        McpServer::command(
            "sh",
            [
                "-c".to_string(),
                SCRIPT.to_string(),
                "sh".to_string(),
                path.to_string_lossy().into_owned(),
            ],
        )
    }

    #[tokio::test]
    async fn a_server_launched_through_a_proxy_serves_its_tools() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("environment");
        let config = McpConfig::default().with_server("notes", echoing(&path));

        let registry = with_defaults_and_mcp_through(
            &config,
            &Proxy::new(CONFIGURED_PROXY).with_bypass(["ollama"]),
        )
        .await;

        assert!(registry.has_mcp());
        assert_eq!(registry.mcp_servers(), ["notes"]);
        let name = crate::mcp::qualified_tool_name("notes", "echo");
        let tool = registry
            .get(&name)
            .expect("the server's tool is registered");
        let result = tool
            .execute(serde_json::json!({}), &crate::tool::ToolContext::default())
            .await
            .expect("the call is answered");
        assert!(result.success, "{result:?}");
        assert!(
            result
                .output
                .as_deref()
                .unwrap_or_default()
                .contains("echoed"),
            "{result:?}"
        );
        assert_routed(
            &std::fs::read_to_string(path).unwrap(),
            CONFIGURED_PROXY,
            "ollama",
        );
    }

    #[tokio::test]
    async fn with_defaults_and_mcp_through_launches_each_server_through_its_proxy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("environment");
        let config = McpConfig::default().with_server("environment", recorder("env", &path));

        let registry = with_defaults_and_mcp_through(
            &config,
            &Proxy::new(CONFIGURED_PROXY).with_bypass(["ollama"]),
        )
        .await;

        assert!(!registry.has_mcp());
        assert_routed(
            &std::fs::read_to_string(path).unwrap(),
            CONFIGURED_PROXY,
            "ollama",
        );
    }
}
