//! When a run stops the processes its agent left behind.

/// When a [`CliProvider`](crate::CliProvider) run stops the processes its
/// agent left behind: whatever is still in the agent's process group once the
/// agent has exited or been stopped, and, when the run is
/// [marked](crate::CliSettings::marker), every process that carries the run's
/// marker wherever it went, a session of its own included.
///
/// Cleaning up kills the group while the agent is still unreaped, so the
/// kill can never reach a group that took over its id, then gives the
/// marked processes [`GRACE_PERIOD`](abnegate_exec::executor::GRACE_PERIOD)
/// to exit on SIGTERM before it kills them. The run's
/// [timeout](crate::CliSettings::timeout) bounds only the agent: a run whose
/// agent exited in time is not failed for a cleanup that outlasts it.
///
/// A run counts as successful here when the agent's stream finished its turn
/// without reporting a failure, nothing stopped the agent, and the agent
/// exited with status zero. The status is read while the agent is still
/// unreaped, so a nonzero exit still reaches the group it led.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cleanup {
    /// Clean up after every run but a successful one, which leaves what the
    /// agent started running, such as a server it started for the caller,
    /// unless it is in the agent's group and still holds the agent's output
    /// open once the grace period after the agent's exit has run out.
    #[default]
    OnFailure,
    /// Clean up after every run, a successful one included, so that no MCP
    /// server or helper that inherited the agent's output keeps running
    /// once the run has answered.
    Always,
}

#[cfg(test)]
mod tests {
    use super::Cleanup;

    #[test]
    fn a_run_leaves_what_a_successful_agent_started_unless_told_otherwise() {
        assert_eq!(Cleanup::default(), Cleanup::OnFailure);
    }
}
