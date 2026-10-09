//! What a run knows once the wait for its agent is over.

/// How the wait for an agent ended, with the agent exited or stopped but
/// still unreaped.
pub(crate) struct Settlement {
    /// Why the run stopped the agent, when it did.
    pub(crate) stopped: Option<String>,
    /// The failure that settled the run while the agent ran, when one did.
    pub(crate) failure: Option<String>,
    /// The status the agent exited with, read before it was reaped, when the
    /// run saw it exit by itself.
    pub(crate) exit: Option<std::process::ExitStatus>,
}
