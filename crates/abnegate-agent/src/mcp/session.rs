use abnegate_exec::Proxy;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool as RemoteTool};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::json;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, Command};
use tokio::sync::Mutex;

use super::name::unique_qualified_tool_name;
use super::tool::McpTool;
use super::{McpError, McpServerSpec};
use crate::tools::Tool;
use crate::tools::process::Group;

/// Most of a server's stderr logged, after which the rest is read and
/// dropped so the server never blocks writing to it.
const MAX_LOGGED_STDERR_BYTES: usize = 64 * 1024;

const STDERR_BUFFER_BYTES: usize = 4 * 1024;

/// One live stdio MCP session.
///
/// The server leads a process group of its own, and dropping the session
/// kills the whole group, so a server that starts helpers of its own leaves
/// none of them behind.
pub(super) struct McpSession {
    pub(super) name: String,
    pub(super) remote_tools: Vec<RemoteTool>,
    client: Mutex<RunningService<RoleClient, ()>>,
    group: Group,
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
            group: Group::led_by(None),
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

    /// The child is given the spec's environment policy, and then the
    /// process-level proxy policy on top of it. Its stderr is logged, up to a
    /// bound, rather than written over this process's own.
    async fn handshake(spec: &McpServerSpec) -> Result<Self, McpError> {
        let mut command = Command::new(&spec.command);
        command.kill_on_drop(true).process_group(0);
        let (transport, stderr) = TokioChildProcess::builder(command.configure(|process| {
            process.args(&spec.arguments);
            spec.environment_policy().apply(process);
            Proxy::from_env().apply(process);
            if let Some(directory) = &spec.working_directory {
                process.current_dir(directory);
            }
        }))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| McpError::Spawn {
            server: spec.name.clone(),
            source,
        })?;
        let group = Group::led_by(transport.id());
        if let Some(stderr) = stderr {
            tokio::spawn(log_stderr(spec.name.clone(), stderr));
        }

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

        let mut session = Self::new(spec.name.clone(), remote_tools, client);
        session.group = group;
        Ok(session)
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

impl Drop for McpSession {
    fn drop(&mut self) {
        self.group.kill();
    }
}

/// Log what a server writes to stderr, up to [`MAX_LOGGED_STDERR_BYTES`],
/// and keep reading past that so it never fills the pipe.
async fn log_stderr(server: String, mut stderr: ChildStderr) {
    let mut buffer = vec![0; STDERR_BUFFER_BYTES];
    let mut logged = 0;
    loop {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if logged >= MAX_LOGGED_STDERR_BYTES {
            continue;
        }
        let kept = read.min(MAX_LOGGED_STDERR_BYTES - logged);
        logged += kept;
        tracing::debug!(
            server = %server,
            stderr = %String::from_utf8_lossy(&buffer[..kept]).trim_end(),
            "MCP server wrote to stderr"
        );
        if logged >= MAX_LOGGED_STDERR_BYTES {
            tracing::debug!(server = %server, "MCP server stderr past its limit; dropping the rest");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CHILD_TEST;
    use abnegate_exec::PROXY_URL_ENV;
    use abnegate_secret::SecretValue;
    use std::collections::BTreeMap;

    /// Set on this test's own child process, where the server under test
    /// would inherit it if nothing stopped it.
    const LEAKED: &str = "ABNEGATE_MCP_ENVIRONMENT_MARKER";

    #[tokio::test]
    async fn proxy_overrides_mcp_environment_before_handshake() {
        const NAME: &str = "mcp::session::tests::proxy_overrides_mcp_environment_before_handshake";
        if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env(CHILD_TEST, NAME)
                .env(PROXY_URL_ENV, "http://127.0.0.1:28888")
                .env(LEAKED, "must-not-reach-a-server")
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
            arguments: vec![
                "-c".to_string(),
                "env > \"$1\"; exec cat > /dev/null".to_string(),
                "sh".to_string(),
                path.to_string_lossy().into_owned(),
            ],
            environment: BTreeMap::from([
                (
                    "HTTPS_PROXY".to_string(),
                    SecretValue::new("http://wrong:8888"),
                ),
                (
                    "http_proxy".to_string(),
                    SecretValue::new("http://wrong:8888"),
                ),
                ("NO_PROXY".to_string(), SecretValue::new("*")),
                ("no_proxy".to_string(), SecretValue::new("*")),
                (PROXY_URL_ENV.to_string(), SecretValue::new("")),
            ]),
            inherit_environment: false,
            working_directory: None,
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
        assert!(
            !output.contains(LEAKED),
            "a variable off the allowlist reached the server: {output}"
        );
    }

    /// A server that starts a helper and then fails its handshake used to
    /// leave the helper running: only the server itself was killed.
    #[tokio::test]
    async fn a_server_that_goes_away_takes_what_it_started_with_it() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("helper");
        let spec = McpServerSpec {
            arguments: vec![
                "-c".to_string(),
                "sleep 30 & echo $! > \"$1\"; exec cat > /dev/null".to_string(),
                "sh".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ],
            ..McpServerSpec::new("helper", "sh", Vec::<String>::new())
        };

        let result = McpSession::connect_with_timeout(&spec, Duration::from_millis(500)).await;
        assert!(matches!(result, Err(McpError::Handshake { .. })));

        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("the server started its helper")
            .trim()
            .parse()
            .expect("a pid");
        let mut gone = false;
        for _ in 0..300 {
            if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(gone, "the helper {pid} outlived its server");
    }
}
