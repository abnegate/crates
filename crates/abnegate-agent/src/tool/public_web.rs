use std::fmt::Debug;
use std::panic::RefUnwindSafe;

/// Whether `run_command` may start `curl` or `wget` right now.
///
/// A host decides this on its own terms: a chat that opted out of the public
/// web, or a tunnel the host requires that is down. `run_command` asks before
/// it starts `curl` or `wget`, in the foreground or the background, and fails
/// the call with the refusal as its message.
///
/// That is all it gates. It is not a network sandbox: other programs on
/// [`DEFAULT_COMMANDS`](crate::tool::DEFAULT_COMMANDS) reach the network too
/// (`git`, `gh`, `npm`, `npx`, `pip`, `uv`, `cargo`), a list passed to
/// [`RunCommandTool::with_commands`](crate::tool::RunCommandTool::with_commands)
/// may name more, and `run_shell` is never asked, so a shell can still run
/// `curl`. A host that must keep a turn off the network withholds those
/// tools or confines the process itself.
///
/// ```
/// use std::sync::Arc;
///
/// use abnegate_agent::ToolContext;
/// use abnegate_agent::tool::PublicWeb;
///
/// #[derive(Debug)]
/// struct Offline;
///
/// impl PublicWeb for Offline {
///     fn refusal(&self) -> Option<String> {
///         Some("This chat does not use the public web.".to_string())
///     }
/// }
///
/// let context = ToolContext::default().with_public_web(Arc::new(Offline));
/// assert!(context.public_web.is_some());
/// ```
pub trait PublicWeb: Debug + RefUnwindSafe + Send + Sync {
    /// Why the public web must not be reached, or nothing when it may be.
    fn refusal(&self) -> Option<String>;
}
