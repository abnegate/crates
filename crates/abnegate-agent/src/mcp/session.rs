use abnegate_exec::Proxy;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool as RemoteTool};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::name::unique_qualified_tool_name;
use super::tool::McpTool;
use super::{McpError, McpServerSpec};
use crate::tools::Tool;

/// One live stdio MCP session.
pub(super) struct McpSession {
    pub(super) name: String,
    pub(super) remote_tools: Vec<RemoteTool>,
    client: Mutex<RunningService<RoleClient, ()>>,
}

impl McpSession {
    pub(super) fn new(
        name: String,
        remote_tools: Vec<RemoteTool>,
        client: RunningService<RoleClient, ()>,
    ) -> Self {
        Self {
            name,
            remote_tools,
            client: Mutex::new(client),
        }
    }

    pub(super) async fn connect_with_timeout(
        spec: &McpServerSpec,
        limit: Duration,
    ) -> Result<Self, McpError> {
        match tokio::time::timeout(limit, Self::handshake(spec)).await {
            Ok(result) => result,
            Err(_) => Err(McpError::Handshake {
                server: spec.name.clone(),
                message: "handshake timed out".to_string(),
            }),
        }
    }

    /// The child inherits this process's environment, overlays the spec's,
    /// and then takes the process-level proxy policy on top of both.
    async fn handshake(spec: &McpServerSpec) -> Result<Self, McpError> {
        let mut command = Command::new(&spec.command);
        command.kill_on_drop(true);
        let transport = TokioChildProcess::new(command.configure(|process| {
            process.args(&spec.args);
            for (key, value) in &spec.env {
                process.env(key, value);
            }
            Proxy::from_env().apply(process);
            if let Some(cwd) = &spec.cwd {
                process.current_dir(cwd);
            }
        }))
        .map_err(|source| McpError::Spawn {
            server: spec.name.clone(),
            source,
        })?;

        let client =
            ().serve(transport)
                .await
                .map_err(|error| McpError::Handshake {
                    server: spec.name.clone(),
                    message: error.to_string(),
                })?;

        let remote_tools = client
            .list_all_tools()
            .await
            .map_err(|error| McpError::Handshake {
                server: spec.name.clone(),
                message: error.to_string(),
            })?;

        Ok(Self::new(spec.name.clone(), remote_tools, client))
    }

    pub(super) async fn call(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, McpError> {
        let client = self.client.lock().await;
        client
            .call_tool(request)
            .await
            .map_err(|error| McpError::Call(error.to_string()))
    }

    /// A tool for every remote tool, named clear of everything in `used`.
    pub(super) fn tools(self: &Arc<Self>, used: &mut HashSet<String>) -> Vec<Arc<dyn Tool>> {
        self.remote_tools
            .iter()
            .map(|tool| {
                let qualified = unique_qualified_tool_name(used, &self.name, tool.name.as_ref());
                let description = tool
                    .description
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .map(|text| format!("[{}] {text}", self.name))
                    .unwrap_or_else(|| format!("[{}] {}", self.name, tool.name));
                let schema = tool.schema_as_json_value();
                let schema = if schema.is_null() {
                    json!({"type": "object", "properties": {}})
                } else {
                    schema
                };
                Arc::new(McpTool::new(
                    qualified,
                    tool.name.to_string(),
                    description,
                    schema,
                    Arc::clone(self),
                )) as Arc<dyn Tool>
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::PROXY_TEST_CHILD;
    use abnegate_exec::PROXY_URL_ENV;
    use std::collections::HashMap;

    #[tokio::test]
    async fn proxy_overrides_mcp_environment_before_handshake() {
        const NAME: &str = "mcp::session::tests::proxy_overrides_mcp_environment_before_handshake";
        if std::env::var(PROXY_TEST_CHILD).as_deref() != Ok(NAME) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env(PROXY_TEST_CHILD, NAME)
                .env(PROXY_URL_ENV, "http://127.0.0.1:28888")
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("environment");
        let spec = McpServerSpec {
            name: "environment".to_string(),
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "env > \"$1\"; exec cat > /dev/null".to_string(),
                "sh".to_string(),
                path.to_string_lossy().into_owned(),
            ],
            env: HashMap::from([
                ("HTTPS_PROXY".to_string(), "http://wrong:8888".to_string()),
                ("http_proxy".to_string(), "http://wrong:8888".to_string()),
                ("NO_PROXY".to_string(), "*".to_string()),
                ("no_proxy".to_string(), "*".to_string()),
                (PROXY_URL_ENV.to_string(), "".to_string()),
            ]),
            cwd: None,
            disabled: false,
        };
        let result = McpSession::connect_with_timeout(&spec, Duration::from_millis(500)).await;
        assert!(matches!(result, Err(McpError::Handshake { .. })));
        let output = std::fs::read_to_string(path).unwrap();
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert!(
                output
                    .lines()
                    .any(|line| line == format!("{key}=http://127.0.0.1:28888")),
                "{output}"
            );
        }
        assert!(
            !output
                .lines()
                .any(|line| line == "NO_PROXY=*" || line == "no_proxy=*")
        );
        assert!(output.contains("NO_PROXY=localhost,127.0.0.1,::1"));
    }
}
