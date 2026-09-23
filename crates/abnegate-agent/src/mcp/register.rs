use std::collections::HashSet;

use super::{McpConfig, McpHub};
use crate::tools::ToolRegistry;

/// Attach every tool from a connected hub to `registry`, and return how many
/// were added.
///
/// Names are prefixed with the server name (`docs_search`). Collisions after
/// sanitizing, including with a tool already registered, are given a numeric
/// suffix so one tool cannot hide another. The hub can be dropped afterwards:
/// each tool holds its own session handle.
pub fn register(registry: &mut ToolRegistry, hub: &McpHub) -> usize {
    for name in hub.server_names() {
        registry.attach(name);
    }
    let mut used: HashSet<String> = registry.names().into_iter().map(str::to_string).collect();
    let tools = hub.tools_avoiding(&mut used);
    let added = tools.len();
    for tool in tools {
        registry.register(tool);
    }
    added
}

/// The default file and command tools plus every server in `config`.
///
/// A server that fails to start is logged and skipped.
pub async fn with_defaults_and_mcp(config: &McpConfig) -> ToolRegistry {
    let mut registry = ToolRegistry::with_defaults();
    let hub = McpHub::connect(config).await;
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
    use crate::mcp::guidance;

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
}
