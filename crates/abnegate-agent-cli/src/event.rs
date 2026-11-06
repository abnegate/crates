//! What one line of a coding agent's output means.

use std::time::Duration;

use abnegate_llm::Limit;
use abnegate_llm::ToolCall;
use abnegate_llm::Usage;
use abnegate_llm::Window;

use crate::occupancy::Occupancy;
use crate::output::Output;
use crate::parser::claude::CliUsage;
use crate::subagent::Subagent;

/// A normalised event, whichever agent produced it.
///
/// A turn a usage limit refused is [`AgentEvent::Limited`] rather than
/// [`AgentEvent::Failed`], so a caller that routes turns between sign-ins can
/// tell an exhausted account from a broken turn without reading prose. Its
/// message is still the agent's own wording, as a failure would carry it,
/// since a caller's retry policy may read that wording.
///
/// A variant may gain a field in a minor release, so a value is built with
/// its constructor, such as [`AgentEvent::finished`], and a pattern outside
/// this crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::AgentEvent;
///
/// fn reason(event: &AgentEvent) -> Option<&str> {
///     match event {
///         AgentEvent::Finished { finish_reason } => finish_reason.as_deref(),
///         _ => None,
///     }
/// }
/// # let _ = reason;
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentEvent {
    /// A block of the agent's prose, in streaming order. Every block after
    /// the first starts with a paragraph break, `\n\n`, so that the blocks
    /// read apart when joined.
    Text(String),
    /// A tool the main agent ran for itself, reported and never replayed. A
    /// subagent's calls are steps of its [`AgentEvent::Subagent`] instead.
    /// Its arguments are the agent's, not scrubbed of the run's secrets.
    Tool(ToolCall),
    /// What one of the agent's own tool calls printed once it finished, a
    /// subagent's included, and whether it worked. It is yielded on the
    /// stream only: never folded into a
    /// [`StdoutParseResult`](crate::StdoutParseResult), and never counted
    /// toward the output limit, since it is the tool's text and not the
    /// agent's prose. Its [text](Output::text) is scrubbed of the run's
    /// secrets and cut to the run's
    /// [`tool_output_limit`](crate::CliSettings::tool_output_limit); its
    /// call's `function.arguments` are not scrubbed, as in
    /// [`AgentEvent::Tool`].
    Output(Output),
    /// Prompt and completion token counts, cache reads and writes counted
    /// as prompt.
    Usage(Usage),
    /// The agent ended its turn in failure, in its own words. The first one
    /// a run reports is its failure.
    Failed(String),
    /// A usage limit refused the agent's turn, which ends it like a
    /// [`AgentEvent::Failed`] with the limit's message, and says when the
    /// limit lifts and which window refused it when the agent said.
    Limited(Limit),
    /// How much of one of the plan's usage windows the account has spent,
    /// reported while the turn goes on. The window keeps the name the agent
    /// streams it under, with its [kind](Window::kind) when it is one of the
    /// plan's.
    Window(Window),
    /// The agent ended its turn, and why, in its own words.
    #[non_exhaustive]
    Finished {
        /// The agent's reason for stopping, such as `end_turn`, when it gave
        /// one.
        finish_reason: Option<String>,
    },
    /// A problem the agent reported without ending its turn, such as a
    /// notice that it is reconnecting. It becomes the run's failure only
    /// when the stream ends without a terminal event.
    Diagnostic(String),
    /// The agent's own identifier for the conversation, which resumes it.
    Session(String),
    /// The answer the agent was asked to shape to a JSON schema.
    Structured(serde_json::Value),
    /// The agent's final answer, apart from the narration it streamed as
    /// [`AgentEvent::Text`] on the way to it: the text of a successful
    /// Claude result. It comes just before the [`AgentEvent::Finished`] it
    /// belongs to and ends nothing itself. A turn that failed has none, and
    /// neither has an agent that reports no answer apart from its prose,
    /// such as Codex, so a caller falls back to the prose, as
    /// [`StdoutParseResult::reply`](crate::StdoutParseResult::reply) does.
    Answer(String),
    /// What the agent says the run cost, in US dollars.
    Cost(f64),
    /// How many turns the agent took.
    Turns(u32),
    /// How long the agent spent waiting on its model's API.
    Latency(Duration),
    /// The counts behind [`AgentEvent::Usage`], with cache reads and cache
    /// writes kept apart from fresh input.
    Tokens(CliUsage),
    /// The account's usage credits carried the turn past the plan's window
    /// this names, which the agent spends without asking. Reported once per
    /// turn.
    Credits(String),
    /// The context window of the model the turn ran on, and how much of it
    /// the turn's last request filled, as the agent reported them. Claude
    /// reports it with the rest of its result, ahead of the
    /// [`AgentEvent::Answer`] and the event that ends the turn. Codex writes
    /// it only to the rollout it keeps of the session, which is read once
    /// the agent has exited by itself, so a Codex stream yields it last,
    /// after the event that ended the turn.
    Occupancy(Occupancy),
    /// What one line of the agent's output said about a subagent it started
    /// for itself: its start, its progress, each of its own tool calls as a
    /// [step](Subagent::step), and how it ended. A background subagent's
    /// news can come after the event that ends the turn.
    ///
    /// Like [`AgentEvent::Output`], it is yielded on the stream only, never
    /// folded into a [`StdoutParseResult`](crate::StdoutParseResult) or
    /// counted toward the output limit, and its words are sanitized and
    /// scrubbed of the run's secrets. Nothing renders it: a caller that shows
    /// a subagent's calls as it shows the main agent's renders each step
    /// itself, with [`Subagent::activity`] for instance, and counts it toward
    /// whatever budget it keeps.
    Subagent(Subagent),
}

impl AgentEvent {
    /// The agent ended its turn for `reason`, in its own words, when it gave
    /// one.
    pub fn finished(reason: Option<String>) -> Self {
        Self::Finished {
            finish_reason: reason,
        }
    }

    /// A tool the agent ran, as the function call it amounts to.
    pub fn tool(id: String, name: String, arguments: String) -> Self {
        Self::Tool(ToolCall::function(id, name, arguments))
    }

    /// Whether this event ends the run, successfully or not.
    ///
    /// An agent that exits zero without emitting one of these never finished
    /// its turn, and treating that as success would hand the caller a silently
    /// truncated answer.
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Failed(_) | Self::Limited(_) | Self::Finished { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use abnegate_llm::Limit;
    use abnegate_llm::Usage;
    use abnegate_llm::Window;
    use abnegate_llm::WindowKind;

    use super::AgentEvent;
    use abnegate_llm::ToolCall;

    use crate::occupancy::Occupancy;
    use crate::output::Output;
    use crate::parser::claude::CliUsage;
    use crate::subagent::Subagent;
    use crate::subagent::SubagentStatus;

    #[test]
    fn only_a_result_ends_the_run() {
        assert!(!AgentEvent::Text("hello".to_string()).terminal());
        assert!(!AgentEvent::Usage(Usage::new(1, 1)).terminal());
        assert!(
            !AgentEvent::Window(
                Window::new("five_hour")
                    .with_used_percent(43.0)
                    .with_kind(WindowKind::FiveHours)
            )
            .terminal()
        );
        assert!(!AgentEvent::Session("6f1".to_string()).terminal());
        assert!(AgentEvent::Failed("nope".to_string()).terminal());
        assert!(AgentEvent::Limited(Limit::new("You've hit your session limit")).terminal());
        assert!(AgentEvent::finished(None).terminal());
    }

    #[test]
    fn a_finished_turn_keeps_its_reason() {
        let AgentEvent::Finished { finish_reason, .. } =
            AgentEvent::finished(Some("success".to_string()))
        else {
            panic!("expected a finished turn");
        };
        assert_eq!(finish_reason.as_deref(), Some("success"));
    }

    #[test]
    fn a_tool_is_reported_as_a_function_call() {
        let AgentEvent::Tool(call) =
            AgentEvent::tool("toolu_01".to_string(), "Read".to_string(), "{}".to_string())
        else {
            panic!("expected a tool call");
        };
        assert_eq!(call.id, "toolu_01");
        assert_eq!(call.call_type, "function");
        assert_eq!(call.function.name, "Read");
        assert_eq!(call.function.arguments, "{}");
    }

    #[test]
    fn run_metadata_never_ends_the_run() {
        for event in [
            AgentEvent::Session("6f1".to_string()),
            AgentEvent::Structured(serde_json::json!({"success": true})),
            AgentEvent::Answer("It is empty.".to_string()),
            AgentEvent::Cost(0.04),
            AgentEvent::Turns(3),
            AgentEvent::Latency(Duration::from_millis(7980)),
            AgentEvent::Tokens(CliUsage::default()),
            AgentEvent::Diagnostic("Reconnecting... 1/5".to_string()),
            AgentEvent::Credits("five_hour".to_string()),
            AgentEvent::Occupancy(Occupancy::reported(200_000, Some(16_632)).expect("a window")),
            AgentEvent::Output(Output::new(
                ToolCall::function("toolu_01", "Bash", "{}"),
                "done",
                true,
            )),
            AgentEvent::Subagent(
                Subagent::new("toolu_01Agent").with_status(SubagentStatus::Completed),
            ),
        ] {
            assert!(!event.terminal(), "{event:?} ended the run");
        }
    }
}
