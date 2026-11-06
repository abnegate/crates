use serde::Deserialize;
use serde::Serialize;

/// Where one of a subagent's own tool calls stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StepStatus {
    /// Made, and not yet answered.
    Running,
    /// Answered with a result that is not an error.
    Succeeded,
    /// Answered with an error.
    Failed,
}

impl StepStatus {
    /// Every status, in the order a call moves through them.
    pub const ALL: &'static [Self] = &[Self::Running, Self::Succeeded, Self::Failed];

    /// The name it is serialized under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// The status serialized as `name`, if any.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::StepStatus;

    #[test]
    fn a_step_status_is_named_in_snake_case_both_ways() {
        for &status in StepStatus::ALL {
            let named = serde_json::to_value(status).expect("a status to serialise");
            assert_eq!(named, serde_json::json!(status.as_str()));
            assert_eq!(
                serde_json::from_value::<StepStatus>(named).ok(),
                Some(status)
            );
            assert_eq!(StepStatus::named(status.as_str()), Some(status));
        }
        assert_eq!(StepStatus::named("Succeeded"), None);
        assert_eq!(StepStatus::named("completed"), None);
    }
}
