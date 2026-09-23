//! Everything an agent's stdout amounted to over one run.

use std::time::Duration;

use abnegate_llm::ToolCall;
use abnegate_llm::Usage;
use serde::de::DeserializeOwned;

use crate::event::AgentEvent;
use crate::parser::claude::CliUsage;

/// The events of one run, folded into what they add up to.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct StdoutParseResult {
    /// The agent's prose, in the order it was streamed.
    pub text: String,
    /// Tools the agent ran for itself. Reported, never replayed.
    pub tools: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub tokens: Option<CliUsage>,
    /// The first failure the agent reported, in its own words.
    pub failure: Option<String>,
    pub finish_reason: Option<String>,
    /// Whether the stream reached a terminal event, successful or not.
    pub finished: bool,
    pub session: Option<String>,
    /// The last schema-shaped answer the agent gave.
    pub structured: Option<serde_json::Value>,
    /// In US dollars.
    pub cost: Option<f64>,
    pub turns: Option<u32>,
    pub latency: Option<Duration>,
}

impl StdoutParseResult {
    pub fn record(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(chunk) => self.text.push_str(&chunk),
            AgentEvent::Tool(call) => self.tools.push(call),
            AgentEvent::Usage(usage) => self.usage = Some(usage),
            AgentEvent::Tokens(tokens) => self.tokens = Some(tokens),
            AgentEvent::Failed(message) => {
                self.finished = true;
                self.failure.get_or_insert(message);
            }
            AgentEvent::Finished { finish_reason } => {
                self.finished = true;
                self.finish_reason = finish_reason;
            }
            AgentEvent::Session(session) => self.session = Some(session),
            AgentEvent::Structured(output) => self.structured = Some(output),
            AgentEvent::Cost(cost) => self.cost = Some(cost),
            AgentEvent::Turns(turns) => self.turns = Some(turns),
            AgentEvent::Latency(latency) => self.latency = Some(latency),
        }
    }

    /// The schema-shaped answer read as `T`, or `None` when there was none or
    /// it does not have that shape.
    pub fn decode<T: DeserializeOwned>(&self) -> Option<T> {
        self.structured
            .as_ref()
            .and_then(|output| T::deserialize(output).ok())
    }
}

impl Extend<AgentEvent> for StdoutParseResult {
    fn extend<I: IntoIterator<Item = AgentEvent>>(&mut self, events: I) {
        for event in events {
            self.record(event);
        }
    }
}

impl FromIterator<AgentEvent> for StdoutParseResult {
    fn from_iter<I: IntoIterator<Item = AgentEvent>>(events: I) -> Self {
        let mut result = Self::default();
        result.extend(events);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use abnegate_llm::Usage;
    use serde_json::json;

    use super::StdoutParseResult;
    use crate::event::AgentEvent;
    use crate::parser::claude::CliUsage;
    use crate::structured_result::StructuredResult;

    #[test]
    fn nothing_recorded_means_nothing_known() {
        let result = StdoutParseResult::default();
        assert!(result.text.is_empty());
        assert!(result.tools.is_empty());
        assert!(result.usage.is_none());
        assert!(result.tokens.is_none());
        assert!(result.failure.is_none());
        assert!(result.finish_reason.is_none());
        assert!(!result.finished);
        assert!(result.session.is_none());
        assert!(result.structured.is_none());
        assert!(result.cost.is_none());
        assert!(result.turns.is_none());
        assert!(result.latency.is_none());
        assert!(format!("{result:?}").contains("StdoutParseResult"));
    }

    #[test]
    fn every_event_lands_in_its_own_field() {
        let result: StdoutParseResult = [
            AgentEvent::Session("sess-1".to_string()),
            AgentEvent::Text("hel".to_string()),
            AgentEvent::Text("lo".to_string()),
            AgentEvent::Structured(json!({"summary": "done", "success": true})),
            AgentEvent::Cost(0.05),
            AgentEvent::Turns(3),
            AgentEvent::Latency(Duration::from_millis(5000)),
            AgentEvent::Tokens(CliUsage {
                input_tokens: Some(100),
                output_tokens: Some(200),
                cache_read_input_tokens: Some(300),
                cache_creation_input_tokens: Some(400),
            }),
            AgentEvent::Usage(Usage {
                prompt_tokens: 800,
                completion_tokens: 200,
                total_tokens: 1000,
            }),
            AgentEvent::Finished {
                finish_reason: Some("success".to_string()),
            },
        ]
        .into_iter()
        .collect();

        assert_eq!(result.text, "hello");
        assert_eq!(result.session.as_deref(), Some("sess-1"));
        assert!((result.cost.expect("a cost") - 0.05).abs() < 1e-6);
        assert_eq!(result.turns, Some(3));
        assert_eq!(result.latency, Some(Duration::from_millis(5000)));
        let tokens = result.tokens.as_ref().expect("token counts");
        assert_eq!(tokens.input_tokens, Some(100));
        assert_eq!(tokens.output_tokens, Some(200));
        assert_eq!(tokens.cache_read_input_tokens, Some(300));
        assert_eq!(tokens.cache_creation_input_tokens, Some(400));
        assert_eq!(
            result.usage.as_ref().map(|usage| usage.total_tokens),
            Some(1000)
        );
        assert_eq!(result.finish_reason.as_deref(), Some("success"));
        assert!(result.finished);
        assert!(result.failure.is_none());

        let report: StructuredResult = result.decode().expect("a report");
        assert!(report.success);
        assert_eq!(report.summary, "done");
    }

    #[test]
    fn the_first_failure_is_kept_and_finishes_the_run() {
        let result: StdoutParseResult = [
            AgentEvent::Failed("You have hit your usage limit.".to_string()),
            AgentEvent::Failed("turn failed".to_string()),
        ]
        .into_iter()
        .collect();

        assert!(result.finished);
        assert_eq!(
            result.failure.as_deref(),
            Some("You have hit your usage limit.")
        );
    }

    #[test]
    fn the_last_structured_answer_and_usage_win() {
        let result: StdoutParseResult = [
            AgentEvent::Structured(json!({"summary": "first", "success": false})),
            AgentEvent::Structured(json!({"summary": "second", "success": true})),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            result
                .decode::<StructuredResult>()
                .expect("a report")
                .summary,
            "second"
        );
    }

    #[test]
    fn an_answer_of_another_shape_does_not_decode() {
        let mut result = StdoutParseResult::default();
        assert!(result.decode::<StructuredResult>().is_none());

        result.record(AgentEvent::Structured(json!({"unrelated": true})));
        assert!(result.decode::<StructuredResult>().is_none());
        assert_eq!(
            result.decode::<serde_json::Value>(),
            Some(json!({"unrelated": true}))
        );
    }
}
