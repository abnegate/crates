use serde::Deserialize;

use crate::subagent::SubagentStatus;

/// Where codex says one of the agents it runs beside itself stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CollabAgentStatus {
    PendingInit,
    Running,
    Interrupted,
    Completed,
    Errored,
    Shutdown,
    NotFound,
}

impl CollabAgentStatus {
    /// What this status says of the subagent. codex reports a `closed` agent
    /// as it stood just before the close, so one still running then was
    /// stopped by it. An agent codex could not find says nothing.
    pub(crate) fn subagent(self, closed: bool) -> Option<SubagentStatus> {
        match self {
            Self::PendingInit | Self::Running if closed => Some(SubagentStatus::Stopped),
            Self::PendingInit | Self::Running => Some(SubagentStatus::Running),
            Self::Completed => Some(SubagentStatus::Completed),
            Self::Errored => Some(SubagentStatus::Failed),
            Self::Interrupted | Self::Shutdown => Some(SubagentStatus::Stopped),
            Self::NotFound => None,
        }
    }
}
