//! The subagents a coding agent starts for itself.

mod step_status;
mod subagent_status;
mod subagent_step;

pub use crate::subagent::step_status::StepStatus;
pub use crate::subagent::subagent_status::SubagentStatus;
pub use crate::subagent::subagent_step::SubagentStep;

use std::time::Duration;

use crate::activity::hidden;
use crate::activity::line;

/// What one line of an agent's output said about one of the subagents it
/// started for itself: Claude's Agent (or, in older releases, Task) tool, or
/// one of the agents Codex runs beside itself through its collaboration
/// tools.
///
/// An update carries only what that line said. Only [`id`](Subagent::id) is
/// always set, and a field left `None` did not change, so a caller folds the
/// updates on one id into what it knows of that subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Subagent {
    /// Claude: the id of the call that started the subagent. Codex: the
    /// subagent's thread.
    pub id: String,
    /// What the subagent was asked to do, in a line.
    pub description: Option<String>,
    /// The kind of subagent, Claude's `subagent_type`.
    pub kind: Option<String>,
    /// The model it runs on: the one it was asked for, then the one it was
    /// given.
    pub model: Option<String>,
    /// Whether it runs in the background, past the main agent's answer.
    pub background: Option<bool>,
    /// Where its run stands.
    pub status: Option<SubagentStatus>,
    /// How many tool calls it has made.
    pub tool_uses: Option<u64>,
    /// How long it has run.
    pub duration: Option<Duration>,
    /// How many tokens it has spent.
    pub tokens: Option<u64>,
    /// The tool it called last.
    pub last_tool: Option<String>,
    /// What it reported, or how it says its work is going: its full report
    /// once its call returns one, which later, shorter summaries never
    /// replace.
    pub summary: Option<String>,
    /// Why it failed, in its own words.
    pub error: Option<String>,
    /// One of its own tool calls, just made or just answered.
    pub step: Option<SubagentStep>,
}

impl Subagent {
    /// An update on subagent `id` that changes nothing else.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// The same update, saying the subagent was asked to do `description`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The same update, saying the subagent is of `kind`.
    #[must_use]
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// The same update, saying the subagent runs on `model`.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// The same update, saying whether the subagent runs in the background.
    #[must_use]
    pub fn with_background(mut self, background: bool) -> Self {
        self.background = Some(background);
        self
    }

    /// The same update, saying the subagent's run stands at `status`.
    #[must_use]
    pub fn with_status(mut self, status: SubagentStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// The same update, saying the subagent has made `tool_uses` calls.
    #[must_use]
    pub fn with_tool_uses(mut self, tool_uses: u64) -> Self {
        self.tool_uses = Some(tool_uses);
        self
    }

    /// The same update, saying the subagent has run for `duration`.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The same update, saying the subagent has spent `tokens`.
    #[must_use]
    pub fn with_tokens(mut self, tokens: u64) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// The same update, saying the subagent last called `tool`.
    #[must_use]
    pub fn with_last_tool(mut self, tool: impl Into<String>) -> Self {
        self.last_tool = Some(tool.into());
        self
    }

    /// The same update, with the subagent's `summary`.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// The same update, saying the subagent failed for `error`.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// The same update, on one of the subagent's own calls.
    #[must_use]
    pub fn with_step(mut self, step: SubagentStep) -> Self {
        self.step = Some(step);
        self
    }

    /// The markdown list item [`activity`](crate::activity) would show for
    /// the call this update says the subagent just made, for a caller that
    /// lists a subagent's calls as it lists the main agent's. An update on
    /// anything else, an answered call included, has none, and so has a call
    /// whose name starts with one of the `hidden` prefixes.
    ///
    /// ```
    /// use abnegate_agent_cli::StepStatus;
    /// use abnegate_agent_cli::Subagent;
    /// use abnegate_agent_cli::SubagentStep;
    ///
    /// let step = SubagentStep::new("toolu_02", "Grep", StepStatus::Running).with_subject("pub mod");
    /// let update = Subagent::new("toolu_01").with_step(step);
    /// assert_eq!(update.activity(&[]).as_deref(), Some("- Grep `pub mod`\n"));
    /// assert_eq!(Subagent::new("toolu_01").activity(&[]), None);
    /// ```
    pub fn activity(&self, hidden_prefixes: &[&str]) -> Option<String> {
        let step = self
            .step
            .as_ref()
            .filter(|step| step.status == StepStatus::Running)?;
        if hidden(&step.name, hidden_prefixes) {
            return None;
        }
        Some(line(&step.name, step.subject.as_deref()))
    }

    /// Whether this update says nothing beyond which subagent it is on.
    pub(crate) fn empty(&self) -> bool {
        *self == Self::new(self.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::StepStatus;
    use super::Subagent;
    use super::SubagentStatus;
    use super::SubagentStep;

    #[test]
    fn a_new_update_names_its_subagent_and_changes_nothing_else() {
        let subagent = Subagent::new("toolu_01Agent");

        assert_eq!(
            subagent,
            Subagent {
                id: "toolu_01Agent".to_string(),
                ..Subagent::default()
            }
        );
        assert!(subagent.empty());
    }

    #[test]
    fn every_builder_sets_its_own_field() {
        let step = SubagentStep::new("toolu_02", "Bash", StepStatus::Running);
        let subagent = Subagent::new("toolu_01")
            .with_description("Review the change")
            .with_kind("general-purpose")
            .with_model("sonnet")
            .with_background(true)
            .with_status(SubagentStatus::Completed)
            .with_tool_uses(3)
            .with_duration(Duration::from_millis(61_000))
            .with_tokens(21_000)
            .with_last_tool("Bash")
            .with_summary("It is sound.")
            .with_error("none")
            .with_step(step.clone());

        assert_eq!(
            subagent,
            Subagent {
                id: "toolu_01".to_string(),
                description: Some("Review the change".to_string()),
                kind: Some("general-purpose".to_string()),
                model: Some("sonnet".to_string()),
                background: Some(true),
                status: Some(SubagentStatus::Completed),
                tool_uses: Some(3),
                duration: Some(Duration::from_secs(61)),
                tokens: Some(21_000),
                last_tool: Some("Bash".to_string()),
                summary: Some("It is sound.".to_string()),
                error: Some("none".to_string()),
                step: Some(step),
            }
        );
        assert!(!subagent.empty());
    }

    #[test]
    fn only_a_call_just_made_and_not_hidden_has_an_activity_line() {
        let running =
            SubagentStep::new("toolu_02", "Bash", StepStatus::Running).with_subject("cargo test");
        assert_eq!(
            Subagent::new("toolu_01")
                .with_step(running)
                .activity(&[])
                .as_deref(),
            Some("- Bash `cargo test`\n")
        );

        let unsubjected = SubagentStep::new("toolu_03", "TodoWrite", StepStatus::Running);
        assert_eq!(
            Subagent::new("toolu_01")
                .with_step(unsubjected)
                .activity(&[])
                .as_deref(),
            Some("- TodoWrite\n")
        );

        for status in [StepStatus::Succeeded, StepStatus::Failed] {
            let answered = SubagentStep::new("toolu_02", "Bash", status);
            assert_eq!(
                Subagent::new("toolu_01").with_step(answered).activity(&[]),
                None,
                "{status:?}"
            );
        }

        let hidden = SubagentStep::new("toolu_04", "mcp__host__search", StepStatus::Running);
        assert_eq!(
            Subagent::new("toolu_01")
                .with_step(hidden)
                .activity(&["mcp__host__"]),
            None
        );
        assert_eq!(
            Subagent::new("toolu_01")
                .with_last_tool("Bash")
                .activity(&[]),
            None
        );
    }
}
