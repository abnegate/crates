//! A loopback listener a tool's command tries to reach, for a test that
//! proves a [`ToolContext`](crate::ToolContext) denied the network keeps its
//! children off it, and that an open one does not.
//!
//! A host that cannot deny a command the network at all is told apart with
//! abnegate-exec's `testing::network_denial_enforced`.
//!
//! ```no_run
//! use abnegate_agent::ToolContext;
//! use abnegate_agent::testing::Loopback;
//! use abnegate_agent::testing::assert_network_denied;
//! use abnegate_agent::tool::Network;
//! use abnegate_agent::tool::RunShellTool;
//! use abnegate_agent::tool::Tool;
//! use serde_json::json;
//!
//! # async fn run() -> std::io::Result<()> {
//! let loopback = Loopback::new()?;
//! let context = ToolContext::default().with_network(Network::Denied);
//! let line = format!("bash '{}'", loopback.script().display());
//!
//! let result = RunShellTool
//!     .execute(json!({"command": line}), &context)
//!     .await
//!     .expect("the shell runs");
//!
//! assert_network_denied(&result.output.unwrap_or_default());
//! # Ok(())
//! # }
//! ```

use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;

use tempfile::TempDir;

/// A live listener on loopback, which holds a connection in its backlog
/// without accepting it, and a bash script that connects to it and prints
/// [`CONNECTED`](Loopback::CONNECTED), or fails when it cannot.
///
/// The script is plain bash rather than `curl` or an interpreter, so only
/// the network its process was started with stands between it and the
/// listener. It runs in the C locale, so [`assert_network_denied`] reads the
/// same refusal whatever locale the host has. Both the listener and the
/// script go when it is dropped.
#[derive(Debug)]
pub struct Loopback {
    _listener: TcpListener,
    _directory: TempDir,
    script: PathBuf,
}

impl Loopback {
    /// What the script prints once it has connected.
    pub const CONNECTED: &str = "connected to the loopback listener";

    /// Bind a listener to a free port on `127.0.0.1` and write the script
    /// that connects to it.
    ///
    /// # Errors
    ///
    /// When the listener cannot be bound or the script cannot be written.
    pub fn new() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let directory = TempDir::new()?;
        let script = directory.path().join("connect.sh");
        std::fs::write(
            &script,
            format!(
                "export LC_ALL=C LANG=C\nexec 3<>/dev/tcp/127.0.0.1/{port} || exit 1\necho '{}'\n",
                Self::CONNECTED
            ),
        )?;
        Ok(Self {
            _listener: listener,
            _directory: directory,
            script,
        })
    }

    /// The script, run as `bash <script>`.
    pub fn script(&self) -> &Path {
        &self.script
    }
}

/// Fail unless `output`, what running a [`Loopback`] script printed, reports
/// a connection the operating system refused, rather than one the listener
/// refused or one that was made: seatbelt answers `EPERM` and the seccomp
/// filter `EACCES`.
///
/// # Panics
///
/// When the script connected, or failed for any other reason.
pub fn assert_network_denied(output: &str) {
    assert!(
        !output.contains(Loopback::CONNECTED),
        "the child reached the listener: {output}"
    );
    let refused = output.lines().any(|line| {
        line.contains("/dev/tcp/")
            && (line.contains("Operation not permitted") || line.contains("Permission denied"))
    });
    assert!(refused, "the child was not refused the network: {output}");
}
