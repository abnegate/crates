//! The agent's own conversation a run starts or carries on.

/// The agent's own session a run starts under an id of the caller's
/// choosing, or resumes, set with
/// [`CliSettings::with_session`](crate::CliSettings::with_session).
///
/// Claude starts a [pinned](Session::pinned) session under the id given, with
/// `--session-id`, and resumes one with `--resume`. Codex names its own
/// threads, so a pinned session leaves its run as it would be without one,
/// and only a [resumed](Session::resumed) one reaches it, as
/// `codex exec resume <id>`. Either agent reports the session it ran in as
/// [`AgentEvent::Session`](crate::AgentEvent::Session). Claude takes only a
/// UUID as a pinned session's id.
///
/// A resumed session already holds the turns it ran, and the run sends the
/// agent every message in the request it is given. Which of a conversation's
/// messages the session has seen is the host's to track, so a host resuming
/// one sends only the messages after them, which
/// [`transcript::render_tail`](crate::transcript::render_tail) renders.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Session {
    /// The session's id, as the agent spells it.
    pub id: String,
    /// Whether the session already exists and the run carries it on, rather
    /// than starting it.
    pub resume: bool,
}

impl Session {
    /// A new session the run starts under `id`.
    pub fn pinned(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resume: false,
        }
    }

    /// The existing session `id`, which the run carries on.
    pub fn resumed(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resume: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Session;

    #[test]
    fn a_pinned_session_starts_and_a_resumed_one_carries_on() {
        let pinned = Session::pinned("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42");
        let resumed = Session::resumed("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42");

        assert_eq!(pinned.id, resumed.id);
        assert!(!pinned.resume);
        assert!(resumed.resume);
    }
}
