//! How the wait for an agent ended.

use crate::verdict::Verdict;

/// The first of the three things a running agent is waited on for.
pub(crate) enum Outcome {
    /// The agent exited, and is left unreaped, with the status it exited
    /// with when that could be read.
    Exited(std::io::Result<Option<std::process::ExitStatus>>),
    TimedOut,
    /// The agent is still running, but its output already settled the run.
    Settled(Verdict),
}
