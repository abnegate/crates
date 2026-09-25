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
