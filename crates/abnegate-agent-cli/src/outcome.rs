//! How the wait for an agent ended.

use crate::verdict::Verdict;

/// The first of the three things a running agent is waited on for.
pub(crate) enum Outcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    /// The agent is still running, but its output already settled the run.
    Settled(Verdict),
}
