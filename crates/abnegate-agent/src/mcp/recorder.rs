use std::path::Path;
use std::time::Duration;

use super::McpServer;

/// Long enough for a loaded machine to start a shell, and never waited out:
/// a [`recorder`] exits once it has written, which ends its handshake.
pub(super) const LIMIT: Duration = Duration::from_secs(30);

/// A server that writes what `script` prints to `path` and then exits
/// without answering, so its handshake fails as soon as the file is written.
pub(super) fn recorder(script: &str, path: &Path) -> McpServer {
    McpServer::command(
        "sh",
        [
            "-c".to_string(),
            format!("{script} > \"$1\""),
            "sh".to_string(),
            path.to_string_lossy().into_owned(),
        ],
    )
}

/// A stdio server that records its environment to the path it is given,
/// then answers the handshake, lists one tool, `echo`, and answers every
/// call to it with `echoed`.
pub(super) fn echoing(path: &Path) -> McpServer {
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
