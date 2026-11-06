use serde::Deserialize;
use serde::Serialize;

use crate::subagent::step_status::StepStatus;

/// One of a subagent's own tool calls, a step of the subagent's run rather
/// than a call of the main agent's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SubagentStep {
    /// The id of the call, which its result is paired with.
    pub id: String,
    /// The tool called.
    pub name: String,
    /// What the call was about, as [`activity`](crate::activity) shows it:
    /// sanitized, and cut to its first line and to 80 characters.
    #[serde(default)]
    pub subject: Option<String>,
    /// Where the call stands.
    pub status: StepStatus,
}

impl SubagentStep {
    /// The call `id` of tool `name`, standing at `status`, about nothing in
    /// particular.
    pub fn new(id: impl Into<String>, name: impl Into<String>, status: StepStatus) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            subject: None,
            status,
        }
    }

    /// The same step, about `subject`.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::SubagentStep;
    use crate::subagent::step_status::StepStatus;

    #[test]
    fn a_step_without_a_subject_still_reads() {
        let step: SubagentStep =
            serde_json::from_str(r#"{"id":"toolu_x","name":"Bash","status":"running"}"#)
                .expect("a step");

        assert_eq!(
            step,
            SubagentStep::new("toolu_x", "Bash", StepStatus::Running)
        );
    }

    #[test]
    fn a_step_writes_its_missing_subject_as_null() {
        let step = SubagentStep::new("toolu_x", "Bash", StepStatus::Succeeded);

        assert_eq!(
            serde_json::to_value(&step).expect("a step to serialise"),
            serde_json::json!({
                "id": "toolu_x",
                "name": "Bash",
                "subject": null,
                "status": "succeeded",
            })
        );
        assert_eq!(step.with_subject("ls").subject.as_deref(), Some("ls"));
    }
}
