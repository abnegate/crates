use serde::Deserialize;
use serde::Serialize;

/// Where a subagent's run stands. Each agent's own words for it are mapped
/// onto these, so a caller reads one set whichever agent started the
/// subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SubagentStatus {
    /// Still working, or waiting to start or to go on.
    Running,
    /// Finished its work.
    Completed,
    /// Ended in failure.
    Failed,
    /// Ended without finishing: it was stopped, or its turn ended first.
    Stopped,
}

impl SubagentStatus {
    /// Every status, in the order a run moves through them.
    pub const ALL: &'static [Self] = &[Self::Running, Self::Completed, Self::Failed, Self::Stopped];

    /// The name it is serialized under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// The status serialized as `name`, if any.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == name)
    }

    /// Whether the run has ended, however it ended.
    pub fn finished(self) -> bool {
        self != Self::Running
    }
}

#[cfg(test)]
mod tests {
    use super::SubagentStatus;

    #[test]
    fn a_status_is_named_in_snake_case_both_ways() {
        for &status in SubagentStatus::ALL {
            let named = serde_json::to_value(status).expect("a status to serialise");
            assert_eq!(named, serde_json::json!(status.as_str()));
            assert_eq!(
                serde_json::from_value::<SubagentStatus>(named).ok(),
                Some(status)
            );
            assert_eq!(SubagentStatus::named(status.as_str()), Some(status));
        }
        assert_eq!(SubagentStatus::named("Running"), None);
        assert_eq!(SubagentStatus::named("errored"), None);
    }

    #[test]
    fn only_a_running_subagent_is_unfinished() {
        for &status in SubagentStatus::ALL {
            assert_eq!(
                status.finished(),
                status != SubagentStatus::Running,
                "{status:?}"
            );
        }
    }
}
