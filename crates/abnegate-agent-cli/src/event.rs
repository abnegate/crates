//! What one line of a coding agent's output means.

use std::time::Duration;

use abnegate_llm::ToolCall;
use abnegate_llm::Usage;

use crate::parser::claude::CliUsage;

/// A normalised event, whichever agent produced it.
///
/// There is deliberately no throttling variant. The caller already owns the
/// vocabulary that separates a throttled run from a rejected one, and it reads
/// that vocabulary out of the failure text. A second classifier here would be
/// a second place to keep in step with it, so a throttled agent simply becomes
/// a [`AgentEvent::Failed`] carrying the agent's own wording.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentEvent {
    Text(String),
    Tool(ToolCall),
    Usage(Usage),
    Failed(String),
    /// The agent ended its turn, and why, in its own words.
    #[non_exhaustive]
    Finished {
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
    /// What the agent says the run cost, in US dollars.
    Cost(f64),
    /// How many turns the agent took.
    Turns(u32),
    /// How long the agent spent waiting on its model's API.
    Latency(Duration),
    /// The counts behind [`AgentEvent::Usage`], with cache reads and cache
    /// writes kept apart from fresh input.
    Tokens(CliUsage),
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
        matches!(self, Self::Failed(_) | Self::Finished { .. })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use abnegate_llm::Usage;

    use super::AgentEvent;
    use crate::parser::claude::CliUsage;

    #[test]
    fn only_a_result_ends_the_run() {
        assert!(!AgentEvent::Text("hello".to_string()).terminal());
        assert!(!AgentEvent::Usage(Usage::new(1, 1)).terminal());
        assert!(AgentEvent::Failed("nope".to_string()).terminal());
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
            AgentEvent::Cost(0.04),
            AgentEvent::Turns(3),
            AgentEvent::Latency(Duration::from_millis(7980)),
            AgentEvent::Tokens(CliUsage::default()),
            AgentEvent::Diagnostic("Reconnecting... 1/5".to_string()),
        ] {
            assert!(!event.terminal(), "{event:?} ended the run");
        }
    }
}
