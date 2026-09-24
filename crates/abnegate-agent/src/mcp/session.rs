use std::collections::HashSet;
use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use abnegate_exec::Proxy;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::Tool as RemoteTool;
use rmcp::service::RunningService;
use rmcp::transport::ConfigureCommandExt;
use rmcp::transport::TokioChildProcess;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::McpError;
use super::McpServer;
use super::name::unique_qualified_tool_name;
use super::tool::McpTool;
use crate::tool::Tool;
use crate::tool::process::Group;

/// Most of a server's stderr logged, after which the rest is read and
/// dropped so the server never blocks writing to it.
const MAXIMUM_LOGGED_STDERR_BYTES: usize = 64 * 1024;

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

    /// Start the command server `server` under `name`, and complete its
    /// handshake within `limit`.
    pub(super) async fn connect_with_timeout(
        name: &str,
        server: &McpServer,
        limit: Duration,
    ) -> Result<Self, McpError> {
        match tokio::time::timeout(limit, Self::handshake(name, server)).await {
            Ok(result) => result,
            Err(_) => Err(McpError::Handshake {
                server: name.to_string(),
                message: "handshake timed out".to_string(),
            }),
        }
    }

    /// The server's references are expanded from this process's environment,
    /// as a CLI expands them. The child is given the server's environment
    /// policy, and then the process-level proxy policy on top of it. Its
    /// stderr is logged, up to a bound, rather than written over this
    /// process's own.
    async fn handshake(name: &str, server: &McpServer) -> Result<Self, McpError> {
        let server = server.expanded(&|variable| std::env::var(variable).ok());
        let Some(program) = &server.command else {
            return Err(McpError::Spawn {
                server: name.to_string(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "no command to launch"),
            });
        };
        let mut command = Command::new(program);
        command.kill_on_drop(true).process_group(0);
        let (transport, stderr) = TokioChildProcess::builder(command.configure(|process| {
            process.args(&server.arguments);
            server.environment_policy().apply(process);
            Proxy::from_environment().apply(process);
            if let Some(directory) = &server.working_directory {
                process.current_dir(directory);
            }
        }))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| McpError::Spawn {
            server: name.to_string(),
            source,
        })?;
        let group = Group::led_by(transport.id());
        if let Some(stderr) = stderr {
            tokio::spawn(log_stderr(name.to_string(), stderr));
        }

        let client =
            ().serve(transport)
                .await
                .map_err(|error| McpError::Handshake {
                    server: name.to_string(),
                    message: error.to_string(),
                })?;

        let remote_tools = client
            .list_all_tools()
            .await
            .map_err(|error| McpError::Handshake {
                server: name.to_string(),
                message: error.to_string(),
            })?;

        let mut session = Self::new(name.to_string(), remote_tools, client);
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

/// Log what a server writes to stderr, up to [`MAXIMUM_LOGGED_STDERR_BYTES`],
/// and keep reading past that so it never fills the pipe.
async fn log_stderr(server: String, mut stderr: ChildStderr) {
    let mut buffer = vec![0; STDERR_BUFFER_BYTES];
    let mut logged = 0;
    loop {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if logged >= MAXIMUM_LOGGED_STDERR_BYTES {
            continue;
        }
        let kept = read.min(MAXIMUM_LOGGED_STDERR_BYTES - logged);
        logged += kept;
        tracing::debug!(
            server = %server,
            stderr = %String::from_utf8_lossy(&buffer[..kept]).trim_end(),
            "MCP server wrote to stderr"
        );
        if logged >= MAXIMUM_LOGGED_STDERR_BYTES {
            tracing::debug!(server = %server, "MCP server stderr past its limit; dropping the rest");
        }
    }
}

#[cfg(test)]
mod tests {
    use abnegate_exec::PROXY_URL_VARIABLE;

    use super::*;
    use crate::test_support::CHILD_TEST;

    /// Set on this test's own child process, where the server under test
    /// would inherit it if nothing stopped it.
    const LEAKED: &str = "ABNEGATE_MCP_ENVIRONMENT_MARKER";

    /// A server that writes what `script` prints to `path` and then never
    /// answers the handshake.
    fn recorder(script: &str, path: &std::path::Path) -> McpServer {
        McpServer::command(
            "sh",
            [
                "-c".to_string(),
                format!("{script} > \"$1\"; exec cat > /dev/null"),
                "sh".to_string(),
                path.to_string_lossy().into_owned(),
            ],
        )
    }

    #[tokio::test]
    async fn proxy_overrides_mcp_environment_before_handshake() {
        const NAME: &str = "mcp::session::tests::proxy_overrides_mcp_environment_before_handshake";
        if std::env::var(CHILD_TEST).as_deref() != Ok(NAME) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env(CHILD_TEST, NAME)
                .env(PROXY_URL_VARIABLE, "http://127.0.0.1:28888")
                .env(LEAKED, "must-not-reach-a-server")
                .output()
                .await
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                stdout.contains("1 passed"),
                "the child ran no test, so it proved nothing\n{stdout}"
            );
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("environment");
        let server = recorder("env", &path)
            .with_environment("HTTPS_PROXY", "http://wrong:8888")
            .with_environment("http_proxy", "http://wrong:8888")
            .with_environment("NO_PROXY", "*")
            .with_environment("no_proxy", "*")
            .with_environment(PROXY_URL_VARIABLE, "");
        let result =
            McpSession::connect_with_timeout("environment", &server, Duration::from_millis(500))
                .await;
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

    /// A CLI is never told a Claude server's working directory, so this is
    /// the one launcher that must honour it.
    #[tokio::test]
    async fn a_server_starts_in_its_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let start = tempfile::tempdir().unwrap();
        let path = directory.path().join("directory");
        let server = recorder("pwd -P", &path).with_working_directory(start.path());

        let result =
            McpSession::connect_with_timeout("directory", &server, Duration::from_millis(500))
                .await;

        assert!(matches!(result, Err(McpError::Handshake { .. })));
        assert_eq!(
            std::fs::read_to_string(path).unwrap().trim_end(),
            start.path().canonicalize().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn a_server_with_no_command_is_never_spawned() {
        let result = McpSession::connect_with_timeout(
            "remote",
            &McpServer::remote("https://example.com/mcp"),
            Duration::from_millis(500),
        )
        .await;

        assert!(
            matches!(
                &result,
                Err(McpError::Spawn { server, source })
                    if server == "remote" && source.kind() == io::ErrorKind::InvalidInput
            ),
            "{:?}",
            result.err()
        );
    }

    /// A server that starts a helper and then fails its handshake used to
    /// leave the helper running: only the server itself was killed.
    #[tokio::test]
    async fn a_server_that_goes_away_takes_what_it_started_with_it() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("helper");
        let server = recorder("sleep 30 & echo $!", &pid_file);

        let result =
            McpSession::connect_with_timeout("helper", &server, Duration::from_millis(500)).await;
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
