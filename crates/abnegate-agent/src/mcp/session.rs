use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use abnegate_exec::Proxy;
use abnegate_http::metadata::is_url;
use reqwest::Url;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::ClientConfig;
use rmcp::model::ProtocolVersion;
use rmcp::model::Tool as RemoteTool;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::McpEndpoint;
use super::McpError;
use super::McpFailure;
use super::McpServer;
use super::cause::Cause;
use super::name::unique_qualified_tool_name;
use super::posted::Posted;
use super::posted::Slot;
use super::remote::Remote;
use super::scrubber::Scrubber;
use super::server_process::ServerProcess;
use super::tool::McpTool;
use super::unconnected::Unconnected;
use crate::tool::Tool;

/// Most of a server's stderr logged, after which the rest is read and
/// dropped so the server never blocks writing to it.
const MAXIMUM_LOGGED_STDERR_BYTES: usize = 64 * 1024;

const STDERR_BUFFER_BYTES: usize = 4 * 1024;

/// Headers the streamable HTTP transport sets itself, which a server's
/// configuration may not.
const RESERVED_HEADERS: [&str; 4] = [
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

const UNRESOLVED: &str = "its URL or headers do not resolve against the server's bound secrets";

const TIMED_OUT: &str = "handshake timed out";

/// The schemes a remote server may be reached over.
const SCHEMES: [&str; 2] = ["http", "https"];

const INVALID_URL: &str = "invalid URL";

const METADATA_URL: &str = "refusing a link-local or cloud metadata address";

/// A client running this crate's [`McpSession::configuration`].
pub(super) type Client = RunningService<RoleClient, ClientConfig>;

/// One live MCP session, over stdio or streamable HTTP.
///
/// A stdio server leads a process group of its own, and dropping the session
/// kills the whole group, so a server that starts helpers of its own leaves
/// none of them behind. The session holds the server itself, not only its
/// group, and never lets it be reaped before that kill: the server's pid
/// keeps the group's id its own however long the session outlives it.
pub(super) struct McpSession {
    pub(super) name: String,
    pub(super) remote_tools: Vec<RemoteTool>,
    client: Mutex<Client>,
    process: Option<ServerProcess>,
    scrubber: Scrubber,
}

impl McpSession {
    pub(super) fn new(name: String, remote_tools: Vec<RemoteTool>, client: Client) -> Self {
        Self {
            name,
            remote_tools,
            client: Mutex::new(client),
            process: None,
            scrubber: Scrubber::default(),
        }
    }

    /// What this client tells every server it speaks: rmcp's own client,
    /// at the newest revision a server can still agree to over `initialize`.
    pub(super) fn configuration() -> ClientConfig {
        ClientConfig::default().with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
    }

    /// Start the command server `server` under `name`, routed through
    /// `proxy`, and complete its handshake within `limit`.
    pub(super) async fn connect_with_timeout(
        name: &str,
        server: &McpServer,
        proxy: &Proxy,
        limit: Duration,
    ) -> Result<Self, Unconnected> {
        match tokio::time::timeout(limit, Self::handshake(name, server, proxy)).await {
            Ok(result) => result,
            Err(_) => Err(Unconnected::handshake(
                name,
                TIMED_OUT,
                Cause::of(McpFailure::Timeout),
            )),
        }
    }

    /// Connect to the remote server `server` under `name` over streamable
    /// HTTP, routed through `proxy`, and complete its handshake and tool
    /// listing within `limit`.
    ///
    /// Its URL and headers resolve against the secrets bound to it alone. A
    /// link-local or metadata address is refused before any request, and so
    /// is every address a name resolves to when the server is reached
    /// directly; behind a proxy, which resolves names itself, only a literal
    /// metadata address is. Only a same-origin 307 or 308 is followed, for at
    /// most [`MAXIMUM_REDIRECTS`](super::redirect::MAXIMUM_REDIRECTS) hops. No failure's text, nor any error
    /// the transport logs itself, holds a URL, a bound secret or a value one
    /// expanded into, as written or percent-encoded.
    pub(super) async fn connect_remote(
        name: &str,
        server: &McpServer,
        proxy: &Proxy,
        limit: Duration,
    ) -> Result<Self, Unconnected> {
        let failed =
            |message: &str, failure: McpFailure| Unconnected::transport(name, message, failure);
        let endpoint = server
            .endpoint()
            .ok_or_else(|| failed(UNRESOLVED, McpFailure::Unresolved))?;
        let url = Url::parse(&endpoint.url)
            .ok()
            .filter(|url| SCHEMES.contains(&url.scheme()))
            .ok_or_else(|| failed(INVALID_URL, McpFailure::Address))?;
        if is_url(&url) {
            return Err(failed(METADATA_URL, McpFailure::Metadata));
        }
        let headers = headers(&endpoint)
            .map_err(|header| failed(&format!("invalid header `{header}`"), McpFailure::Header))?;
        let scrubber = Scrubber::new(server, &endpoint);
        let client = Remote::new(proxy, scrubber.clone())
            .map_err(|message| failed(&message, McpFailure::Client))?;
        let posted = client.posted();
        let transport = StreamableHttpClientTransport::with_client(
            client,
            StreamableHttpClientTransportConfig::with_uri(String::from(url))
                .custom_headers(headers),
        );
        let connected = async {
            let client = Self::configuration()
                .serve(transport)
                .await
                .map_err(|error| {
                    let cause = Cause::initialize(&error, Posted::read(&posted));
                    Unconnected::handshake(name, error.to_string(), cause)
                })?;
            Self::listed(name, server, client, None, Some(posted.as_ref())).await
        };
        let result = tokio::time::timeout(limit, connected)
            .await
            .unwrap_or_else(|_| {
                Err(Unconnected::handshake(
                    name,
                    TIMED_OUT,
                    Cause::of(McpFailure::Timeout),
                ))
            });
        match result {
            Ok(mut session) => {
                session.scrubber = scrubber;
                Ok(session)
            }
            Err(Unconnected { error, cause }) => Err(Unconnected {
                error: scrubbed_error(error, &scrubber),
                cause,
            }),
        }
    }

    /// The server's references are expanded from the secrets bound to it and
    /// then this process's environment, as a CLI expands them. The child is
    /// given the server's environment policy, and then `proxy` on top of it.
    /// Its stderr is logged, up to a bound, rather than written over this
    /// process's own.
    async fn handshake(name: &str, server: &McpServer, proxy: &Proxy) -> Result<Self, Unconnected> {
        let server = server.expanded(&|variable| std::env::var(variable).ok());
        let Some(program) = &server.command else {
            return Err(Unconnected::spawn(
                name,
                io::Error::new(io::ErrorKind::InvalidInput, "no command to launch"),
            ));
        };
        let mut command = Command::new(program);
        command
            .args(&server.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        server.environment_policy().apply(&mut command);
        proxy.apply(&mut command);
        if let Some(directory) = &server.working_directory {
            command.current_dir(directory);
        }
        let mut child = command
            .spawn()
            .map_err(|source| Unconnected::spawn(name, source))?;
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let process = ServerProcess::new(child);
        if let Some(stderr) = stderr {
            tokio::spawn(log_stderr(name.to_string(), stderr));
        }
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            return Err(Unconnected::spawn(
                name,
                io::Error::other("the server was started without its pipes"),
            ));
        };

        let transport =
            AsyncRwTransport::new_client(stdout, stdin).with_max_line_length(usize::MAX);
        let client = Self::configuration()
            .serve(transport)
            .await
            .map_err(|error| {
                let cause = Cause::initialize(&error, Posted::default());
                Unconnected::handshake(name, error.to_string(), cause)
            })?;

        Self::listed(name, &server, client, Some(process), None).await
    }

    /// The session for `server`, whose handshake `client` has completed, with
    /// the tools its server lists that `server` [allows](McpServer::allows),
    /// as a CLI allows them.
    ///
    /// `posted` is where a remote server's client keeps what became of its
    /// last request, read only once the listing has failed; a session over
    /// anything else has none.
    pub(super) async fn listed(
        name: &str,
        server: &McpServer,
        client: Client,
        process: Option<ServerProcess>,
        posted: Option<&Slot>,
    ) -> Result<Self, Unconnected> {
        let remote_tools = client
            .list_all_tools()
            .await
            .map_err(|error| {
                let posted = posted.map(Posted::read).unwrap_or_default();
                Unconnected::handshake(name, error.to_string(), Cause::service(&error, posted))
            })?
            .into_iter()
            .filter(|tool| server.allows(&tool.name))
            .collect();

        let mut session = Self::new(name.to_string(), remote_tools, client);
        session.process = process;
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
            .map_err(|error| McpError::Call(self.scrubber.scrubbed(&error.to_string())))
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

/// Each of `endpoint`'s headers as the transport sends it, marked sensitive,
/// or the name of the first one that is not a valid header, is one the
/// transport sets itself, or repeats another's name in any case.
fn headers(endpoint: &McpEndpoint) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    let mut headers = HashMap::with_capacity(endpoint.headers.len());
    for (name, value) in &endpoint.headers {
        let header = HeaderName::from_bytes(name.as_bytes())
            .ok()
            .filter(|header| !RESERVED_HEADERS.contains(&header.as_str()))
            .ok_or_else(|| name.clone())?;
        let mut value = HeaderValue::from_str(value.expose()).map_err(|_| name.clone())?;
        value.set_sensitive(true);
        if headers.insert(header, value).is_some() {
            return Err(name.clone());
        }
    }
    Ok(headers)
}

/// `error` with its message scrubbed by `scrubber`.
fn scrubbed_error(error: McpError, scrubber: &Scrubber) -> McpError {
    match error {
        McpError::Handshake { server, message } => McpError::Handshake {
            server,
            message: scrubber.scrubbed(&message),
        },
        McpError::Transport { server, message } => McpError::Transport {
            server,
            message: scrubber.scrubbed(&message),
        },
        McpError::Call(message) => McpError::Call(scrubber.scrubbed(&message)),
        error => error,
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
    use std::path::Path;

    use abnegate_exec::PROXY_URL_VARIABLE;

    use super::*;
    use crate::mcp::recorder::LIMIT;
    use crate::mcp::recorder::recorder;
    use crate::test_support::CHILD_TEST;
    use crate::test_support::PATIENCE;
    use crate::test_support::assert_passed;

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
                .env(PROXY_URL_VARIABLE, "http://127.0.0.1:28888")
                .env(LEAKED, "must-not-reach-a-server")
                .output()
                .await
                .unwrap();
            assert_passed(&output);
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
        let result = McpSession::connect_with_timeout(
            "environment",
            &server,
            &Proxy::from_environment(),
            LIMIT,
        )
        .await;
        assert!(matches!(
            result,
            Err(Unconnected {
                error: McpError::Handshake { .. },
                ..
            })
        ));
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
            McpSession::connect_with_timeout("directory", &server, &Proxy::default(), LIMIT).await;

        assert!(matches!(
            result,
            Err(Unconnected {
                error: McpError::Handshake { .. },
                ..
            })
        ));
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
            &Proxy::default(),
            LIMIT,
        )
        .await;

        assert!(
            matches!(
                &result,
                Err(Unconnected {
                    error: McpError::Spawn { server, source },
                    ..
                }) if server == "remote" && source.kind() == io::ErrorKind::InvalidInput
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
        let server = recorder("sleep 30 > /dev/null 2>&1 & echo $!", &pid_file);

        let result =
            McpSession::connect_with_timeout("helper", &server, &Proxy::default(), LIMIT).await;
        assert!(matches!(
            result,
            Err(Unconnected {
                error: McpError::Handshake { .. },
                ..
            })
        ));

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

    /// A server that runs `script` in `sh`, given `path` as `$1`.
    fn scripted(script: &str, path: &Path) -> McpServer {
        McpServer::command(
            "sh",
            [
                "-c".to_string(),
                script.to_string(),
                "sh".to_string(),
                path.to_string_lossy().into_owned(),
            ],
        )
    }

    /// A stdio server that answers the handshake and an empty tool listing,
    /// then leaves a helper in its group and exits, having written its own
    /// pid and the helper's to the path it is given.
    fn lingering(path: &Path) -> McpServer {
        const SCRIPT: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"lingering","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      (while :; do sleep 1; done) < /dev/null > /dev/null 2>&1 &
      printf '%s %s\n' "$$" "$!" > "$1.partial" && mv "$1.partial" "$1"
      exit 0 ;;
  esac
done"#;
        scripted(SCRIPT, path)
    }

    /// A stdio server that agrees to whatever revision its `initialize`
    /// offers, as many servers do, having written that revision to the path
    /// it is given, and then lists no tools.
    fn echoing(path: &Path) -> McpServer {
        const SCRIPT: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '%s' "$version" > "$1.partial" && mv "$1.partial" "$1"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"echoing","version":"1"}}}\n' "$id" "$version" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id" ;;
  esac
done"#;
        scripted(SCRIPT, path)
    }

    /// rmcp's default revision became one with no `initialize` handshake, so
    /// a server that agreed to what it was offered left the session speaking
    /// per-request metadata to a server that knows only the handshake.
    #[tokio::test]
    async fn a_server_is_offered_a_revision_that_has_a_handshake() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("version");

        let session =
            McpSession::connect_with_timeout("echoing", &echoing(&path), &Proxy::default(), LIMIT)
                .await
                .expect("the handshake completes");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "2025-11-25");
        let negotiated = session
            .client
            .lock()
            .await
            .peer_info()
            .map(|info| info.protocol_version.to_string());
        assert_eq!(negotiated.as_deref(), Some("2025-11-25"));
    }

    /// A stdio server with one tool, `dump`, whose every call is answered by
    /// a single message of the size `$1` holds, in bytes.
    fn dumping(path: &Path) -> McpServer {
        const SCRIPT: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"dumping","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"dump","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"' "$id"
      head -c "$(cat "$1")" /dev/zero | tr '\0' x
      printf '"}]}}\n' ;;
  esac
done"#;
        scripted(SCRIPT, path)
    }

    /// rmcp 3.6 began dropping any stdio message past 16 MiB and leaving the
    /// call that awaited it to its command timeout, where a server's reply of
    /// any size used to arrive and be cut down to what a model is shown.
    #[tokio::test]
    async fn a_reply_past_sixteen_mebibytes_still_arrives() {
        const SIZE: usize = 17 * 1024 * 1024;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("size");
        std::fs::write(&path, SIZE.to_string()).unwrap();
        let session =
            McpSession::connect_with_timeout("dumping", &dumping(&path), &Proxy::default(), LIMIT)
                .await
                .expect("the handshake completes");

        let result = tokio::time::timeout(LIMIT, session.call(CallToolRequestParams::new("dump")))
            .await
            .expect("the reply arrives")
            .expect("the call succeeds");

        let text = result.content[0].as_text().map(|text| text.text.len());
        assert_eq!(text, Some(SIZE));
    }

    /// The state `ps` gives `pid`, `Z` for a zombie, or `None` once it has
    /// been reaped.
    fn state(pid: i32) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("a process listing");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!state.is_empty()).then_some(state)
    }

    async fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while !condition() {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }

    /// A server that exited by itself was reaped as soon as its transport
    /// closed, while its session went on holding the group it led, for as
    /// long as the session lived, and a kill it sent later could reach
    /// whatever group had been given that id since. The session keeps the
    /// server unreaped, holding the id, until the group has been killed.
    #[tokio::test(flavor = "current_thread")]
    async fn a_server_is_reaped_only_once_its_group_has_been_killed() {
        let directory = tempfile::tempdir().unwrap();
        let pids = directory.path().join("pids");

        let session = McpSession::connect_with_timeout(
            "lingering",
            &lingering(&pids),
            &Proxy::default(),
            LIMIT,
        )
        .await
        .expect("the handshake completes");
        assert!(
            eventually(|| pids.exists()).await,
            "the server never listed its tools"
        );
        let written = std::fs::read_to_string(&pids).unwrap();
        let (server, helper) = written
            .trim()
            .split_once(' ')
            .map(|(server, helper)| {
                (
                    server.parse::<i32>().unwrap(),
                    helper.parse::<i32>().unwrap(),
                )
            })
            .expect("two pids");
        let closed = eventually(|| {
            session
                .client
                .try_lock()
                .is_ok_and(|client| client.is_transport_closed())
        })
        .await;
        assert!(closed, "the session never saw its server go");
        assert!(
            eventually(|| state(server).is_none_or(|state| state.starts_with('Z'))).await,
            "the server never exited"
        );

        assert!(
            state(server).is_some_and(|state| state.starts_with('Z')),
            "the server was reaped while its session could still signal its group"
        );
        assert!(
            state(helper).is_some(),
            "the helper went before the session"
        );

        drop(session);

        assert!(
            eventually(|| state(helper).is_none()).await,
            "dropping the session left the helper {helper} running"
        );
        assert!(
            eventually(|| state(server).is_none()).await,
            "dropping the session never reaped the server"
        );
    }

    /// A stdio server that completes the handshake and then answers its
    /// tool listing with a JSON-RPC error.
    fn rejecting(path: &Path) -> McpServer {
        const SCRIPT: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"rejecting","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"no tools here"}}\n' "$id" ;;
  esac
done"#;
        scripted(SCRIPT, path)
    }

    /// Why `result` is not a session.
    fn cause(result: Result<McpSession, Unconnected>) -> Cause {
        result.err().expect("the server is not connected").cause
    }

    #[tokio::test]
    async fn a_command_server_whose_listing_is_rejected_names_why() {
        let directory = tempfile::tempdir().unwrap();
        let server = rejecting(&directory.path().join("unused"));

        let result =
            McpSession::connect_with_timeout("docs", &server, &Proxy::default(), LIMIT).await;

        assert_eq!(cause(result), Cause::of(McpFailure::Rejected));
    }
}
