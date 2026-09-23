//! What an agent's own output decided about a run while it was still running.

/// The first terminal word from a reader, which settles the run whether or
/// not the process goes on to exit by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The agent completed its turn.
    Finished,
    /// The agent reported a failure, a diagnostic tripped the caller's
    /// tripwire, or the output broke one of the stream's limits.
    Failed(String),
}
