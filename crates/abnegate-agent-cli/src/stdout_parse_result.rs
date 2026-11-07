//! Everything an agent's stdout amounted to over one run.

use std::time::Duration;

use abnegate_llm::Limit;
use abnegate_llm::ToolCall;
use abnegate_llm::Usage;
use abnegate_llm::Window;
use serde::de::DeserializeOwned;

use crate::event::AgentEvent;
use crate::occupancy::Occupancy;
use crate::paragraphs::PARAGRAPH_BREAK;
use crate::parser::claude::CliUsage;

/// The events of one run, folded into what they add up to.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct StdoutParseResult {
    /// The agent's prose, in the order it was streamed: its narration and
    /// its answer alike.
    pub text: String,
    /// The agent's final answer, apart from the narration in
    /// [`text`](StdoutParseResult::text), when it reported one, or each of its
    /// turns' answers as paragraphs when it ran more than one.
    pub answer: Option<String>,
    /// Tools the main agent ran for itself. Reported, never replayed. A
    /// subagent's calls are not among them.
    pub tools: Vec<ToolCall>,
    /// The token counts reported, added up over the run's results, cache
    /// reads and writes counted as prompt.
    pub usage: Option<Usage>,
    /// The raw token counts reported, added up over the run's results, with
    /// the cache kept apart.
    pub tokens: Option<CliUsage>,
    /// The first failure the agent reported, in its own words, or the last
    /// [diagnostic](StdoutParseResult::diagnostic) of a stream that ended
    /// without a terminal event.
    pub failure: Option<String>,
    /// The last problem the agent reported without ending its turn.
    pub diagnostic: Option<String>,
    /// Why the agent ended its turn, in its own words, when it said.
    pub finish_reason: Option<String>,
    /// Whether the stream reached a terminal event, successful or not.
    pub finished: bool,
    /// The agent's identifier for the conversation, which resumes it.
    pub session: Option<String>,
    /// The last schema-shaped answer the agent gave.
    pub structured: Option<serde_json::Value>,
    /// In US dollars.
    pub cost: Option<f64>,
    /// How many turns the agent took, added up over the run's results.
    pub turns: Option<u32>,
    /// How long the agent spent waiting on its model's API.
    pub latency: Option<Duration>,
    /// Events too long to read that the run could do without, such as a
    /// tool result holding an image, which were skipped.
    pub dropped: u64,
    /// The plan's window the account's usage credits carried the run past,
    /// when the agent said they did.
    pub credits: Option<String>,
    /// The usage limit that refused the turn, when the run's
    /// [failure](StdoutParseResult::failure) was one: its message is that
    /// failure.
    pub limit: Option<Limit>,
    /// The latest report on each of the plan's usage windows, in the order
    /// each was first reported.
    pub windows: Vec<Window>,
    /// The context window of the model the turn ran on, and how much of it
    /// the turn filled, the last time the agent reported them.
    pub occupancy: Option<Occupancy>,
    /// [`usage`](StdoutParseResult::usage) as of the last finished turn.
    earlier_usage: Option<Usage>,
    /// [`tokens`](StdoutParseResult::tokens) as of the last finished turn.
    earlier_tokens: Option<CliUsage>,
    /// [`answer`](StdoutParseResult::answer) as of the last finished turn.
    earlier_answer: Option<String>,
    /// Whether the output ended with a turn begun after one finished and not
    /// finished itself.
    ended_mid_turn: bool,
}

impl StdoutParseResult {
    /// Fold one event in. Prose and tool calls accumulate, the first failure,
    /// a limit's included, and the first credit window are kept, a window
    /// report replaces the last one on the same window, a tool's
    /// [output](AgentEvent::Output) and news of a
    /// [subagent](AgentEvent::Subagent) are left to the stream, and every
    /// other field takes the latest value reported.
    ///
    /// Within a turn that goes for usage, tokens and the answer too, since a
    /// result restates what the turn's progress reported. A run's further
    /// turns report only themselves, so across a
    /// [finished](AgentEvent::Finished) turn usage, tokens and the turn count
    /// add up, and answers join as paragraphs.
    pub fn record(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(chunk) => self.text.push_str(&chunk),
            AgentEvent::Tool(call) => self.tools.push(call),
            AgentEvent::Usage(usage) => {
                self.usage = Some(match self.earlier_usage {
                    Some(spent) => spent.plus(usage),
                    None => usage,
                });
            }
            AgentEvent::Tokens(tokens) => {
                self.tokens = Some(match self.earlier_tokens.clone() {
                    Some(spent) => spent.plus(tokens),
                    None => tokens,
                });
            }
            AgentEvent::Failed(message) => {
                self.finished = true;
                self.failure.get_or_insert(message);
            }
            AgentEvent::Limited(limit) => {
                self.finished = true;
                if self.failure.is_none() {
                    self.failure = Some(limit.message.clone());
                    self.limit = Some(limit);
                }
            }
            AgentEvent::Window(window) => {
                match self
                    .windows
                    .iter_mut()
                    .find(|known| known.name == window.name)
                {
                    Some(known) => *known = window,
                    None => self.windows.push(window),
                }
            }
            AgentEvent::Finished { finish_reason } => {
                self.finished = true;
                self.finish_reason = finish_reason;
                self.earlier_usage = self.usage;
                self.earlier_tokens.clone_from(&self.tokens);
                self.earlier_answer.clone_from(&self.answer);
            }
            AgentEvent::Session(session) => self.session = Some(session),
            AgentEvent::Structured(output) => self.structured = Some(output),
            AgentEvent::Answer(answer) => {
                self.answer = Some(match &self.earlier_answer {
                    Some(earlier) => format!("{earlier}{PARAGRAPH_BREAK}{answer}"),
                    None => answer,
                });
            }
            AgentEvent::Cost(cost) => self.cost = Some(cost),
            AgentEvent::Turns(turns) => {
                self.turns = Some(self.turns.unwrap_or_default().saturating_add(turns));
            }
            AgentEvent::Latency(latency) => self.latency = Some(latency),
            AgentEvent::Diagnostic(message) => self.diagnostic = Some(message),
            AgentEvent::Credits(window) => {
                self.credits.get_or_insert(window);
            }
            AgentEvent::Occupancy(occupancy) => self.occupancy = Some(occupancy),
            AgentEvent::Output(_) | AgentEvent::Subagent(_) => {}
        }
    }

    /// Settle what a stream that has ended amounted to: one that ended
    /// without a terminal event failed with the last diagnostic the agent
    /// reported, when it reported any.
    pub fn conclude(&mut self) {
        if !self.finished && self.failure.is_none() {
            self.failure.clone_from(&self.diagnostic);
        }
    }

    /// How long the answers of the finished turns are, when any gave one.
    pub(crate) fn earlier_answer_len(&self) -> Option<usize> {
        self.earlier_answer.as_ref().map(String::len)
    }

    /// Note that the output ended with a later turn still being written.
    pub(crate) fn end_mid_turn(&mut self) {
        self.ended_mid_turn = true;
    }

    /// Whether the output ended with a later turn still being written.
    pub(crate) fn ended_mid_turn(&self) -> bool {
        self.ended_mid_turn
    }

    /// Whether a finished turn said anything: an answer, or prose.
    pub(crate) fn answered(&self) -> bool {
        self.finished && !self.reply().trim().is_empty()
    }

    /// What the agent answered: its final [answer](StdoutParseResult::answer)
    /// when it reported one that is not blank, or else all the
    /// [prose](StdoutParseResult::text) it streamed.
    pub fn reply(&self) -> &str {
        self.answer
            .as_deref()
            .filter(|answer| !answer.trim().is_empty())
            .unwrap_or(&self.text)
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

    use abnegate_llm::Limit;
    use abnegate_llm::Usage;
    use abnegate_llm::Window;
    use serde_json::json;

    use super::StdoutParseResult;
    use crate::event::AgentEvent;
    use crate::occupancy::Occupancy;
    use crate::parser::claude::CliUsage;
    use crate::structured_result::StructuredResult;

    #[test]
    fn nothing_recorded_means_nothing_known() {
        let result = StdoutParseResult::default();
        assert!(result.text.is_empty());
        assert!(result.answer.is_none());
        assert_eq!(result.reply(), "");
        assert!(result.tools.is_empty());
        assert!(result.usage.is_none());
        assert!(result.tokens.is_none());
        assert!(result.failure.is_none());
        assert!(result.diagnostic.is_none());
        assert!(result.finish_reason.is_none());
        assert!(!result.finished);
        assert!(result.session.is_none());
        assert!(result.structured.is_none());
        assert!(result.cost.is_none());
        assert!(result.turns.is_none());
        assert!(result.latency.is_none());
        assert_eq!(result.dropped, 0);
        assert!(result.credits.is_none());
        assert!(result.limit.is_none());
        assert!(result.windows.is_empty());
        assert!(result.occupancy.is_none());
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
            AgentEvent::Usage(Usage::new(800, 200)),
            AgentEvent::Answer("lo".to_string()),
            AgentEvent::Finished {
                finish_reason: Some("success".to_string()),
            },
        ]
        .into_iter()
        .collect();

        assert_eq!(result.text, "hello");
        assert_eq!(result.answer.as_deref(), Some("lo"));
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

    const NARRATION: &str = "#42 returned 500 — LIVE FAIL, retrying.";
    const FINAL_REPORT: &str = "- #42 fix uploads LIVE PASS\nDEPLOY_QA_VERDICT: ALL_VERIFIED";

    fn narrated(answer: Option<&str>) -> StdoutParseResult {
        [
            AgentEvent::Text(NARRATION.to_string()),
            AgentEvent::Text(FINAL_REPORT.to_string()),
        ]
        .into_iter()
        .chain(answer.map(|answer| AgentEvent::Answer(answer.to_string())))
        .chain([AgentEvent::finished(Some("success".to_string()))])
        .collect()
    }

    #[test]
    fn the_reply_is_only_the_final_answer() {
        let result = narrated(Some(FINAL_REPORT));

        assert_eq!(result.reply(), FINAL_REPORT);
        assert_eq!(result.answer.as_deref(), Some(FINAL_REPORT));
        assert_eq!(result.text, format!("{NARRATION}{FINAL_REPORT}"));
    }

    #[test]
    fn the_reply_falls_back_to_the_prose_without_a_final_answer() {
        let result = narrated(None);

        assert_eq!(result.reply(), format!("{NARRATION}{FINAL_REPORT}"));
    }

    #[test]
    fn the_reply_falls_back_to_the_prose_for_a_blank_final_answer() {
        let result = narrated(Some(" \n "));

        assert_eq!(result.reply(), format!("{NARRATION}{FINAL_REPORT}"));
    }

    #[test]
    fn a_final_answer_ends_nothing_and_the_last_one_wins() {
        let result: StdoutParseResult = [
            AgentEvent::Answer("first".to_string()),
            AgentEvent::Answer("second".to_string()),
        ]
        .into_iter()
        .collect();

        assert!(!result.finished);
        assert!(result.failure.is_none());
        assert_eq!(result.answer.as_deref(), Some("second"));
    }

    #[test]
    fn each_finished_turn_adds_its_own_usage_turns_and_answer() {
        let result: StdoutParseResult = [
            AgentEvent::Usage(Usage::new(5, 1)),
            AgentEvent::Usage(Usage::new(20, 10)),
            AgentEvent::Turns(2),
            AgentEvent::Answer("First.".to_string()),
            AgentEvent::finished(Some("success".to_string())),
            AgentEvent::Usage(Usage::new(4, 1)),
            AgentEvent::Usage(Usage::new(10, 5)),
            AgentEvent::Turns(1),
            AgentEvent::Answer("Second.".to_string()),
            AgentEvent::finished(Some("success".to_string())),
        ]
        .into_iter()
        .collect();

        let usage = result.usage.expect("the run's usage");
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (30, 15));
        assert_eq!(result.turns, Some(3));
        assert_eq!(result.answer.as_deref(), Some("First.\n\nSecond."));
    }

    #[test]
    fn cache_tokens_add_up_across_finished_turns() {
        let result: StdoutParseResult = [
            AgentEvent::Tokens(
                CliUsage::new(10, 4)
                    .with_cache_read_input_tokens(100)
                    .with_cache_creation_input_tokens(20),
            ),
            AgentEvent::finished(Some("success".to_string())),
            AgentEvent::Tokens(
                CliUsage::new(5, 2)
                    .with_cache_read_input_tokens(50)
                    .with_cache_creation_input_tokens(7),
            ),
            AgentEvent::finished(Some("success".to_string())),
        ]
        .into_iter()
        .collect();

        let tokens = result.tokens.expect("the run's tokens");
        assert_eq!(tokens.input_tokens, Some(15));
        assert_eq!(tokens.output_tokens, Some(6));
        assert_eq!(tokens.cache_read_input_tokens, Some(150));
        assert_eq!(tokens.cache_creation_input_tokens, Some(27));
        assert_eq!(tokens.prompt_tokens(), 15 + 150 + 27);
    }

    #[test]
    fn a_turn_still_open_counts_its_latest_progress_over_the_finished_ones() {
        let result: StdoutParseResult = [
            AgentEvent::Usage(Usage::new(20, 10)),
            AgentEvent::finished(Some("success".to_string())),
            AgentEvent::Usage(Usage::new(3, 1)),
            AgentEvent::Usage(Usage::new(6, 2)),
        ]
        .into_iter()
        .collect();

        let usage = result.usage.expect("the run's usage");
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (26, 12));
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
    fn a_diagnostic_followed_by_a_finished_turn_is_not_a_failure() {
        let mut result: StdoutParseResult = [
            AgentEvent::Diagnostic("Reconnecting... 1/5".to_string()),
            AgentEvent::Text("Done.".to_string()),
            AgentEvent::Finished {
                finish_reason: Some("completed".to_string()),
            },
        ]
        .into_iter()
        .collect();
        result.conclude();

        assert!(result.finished);
        assert!(result.failure.is_none());
        assert_eq!(result.diagnostic.as_deref(), Some("Reconnecting... 1/5"));
    }

    #[test]
    fn an_unfinished_stream_fails_with_its_last_diagnostic() {
        let mut result: StdoutParseResult = [
            AgentEvent::Diagnostic("Reconnecting... 1/5".to_string()),
            AgentEvent::Diagnostic("stream disconnected before completion".to_string()),
        ]
        .into_iter()
        .collect();
        assert!(
            result.failure.is_none(),
            "a diagnostic alone fails nothing yet"
        );

        result.conclude();

        assert!(!result.finished);
        assert_eq!(
            result.failure.as_deref(),
            Some("stream disconnected before completion")
        );
    }

    #[test]
    fn a_reported_failure_outranks_a_diagnostic() {
        let mut result: StdoutParseResult = [
            AgentEvent::Diagnostic("Reconnecting... 1/5".to_string()),
            AgentEvent::Failed("You have hit your usage limit.".to_string()),
        ]
        .into_iter()
        .collect();
        result.conclude();

        assert_eq!(
            result.failure.as_deref(),
            Some("You have hit your usage limit.")
        );
    }

    #[test]
    fn the_first_window_usage_credits_carried_the_run_past_is_kept() {
        let result: StdoutParseResult = [
            AgentEvent::Credits("five_hour".to_string()),
            AgentEvent::Credits("seven_day".to_string()),
        ]
        .into_iter()
        .collect();

        assert_eq!(result.credits.as_deref(), Some("five_hour"));
        assert!(!result.finished);
        assert!(result.failure.is_none());
    }

    #[test]
    fn a_limit_finishes_the_run_as_its_failure() {
        let limit = Limit::new("rate limit reached (five_hour, rejected): You've hit your limit")
            .with_window(Window::new("five_hour").with_used_percent(100.0));
        let mut result: StdoutParseResult = [
            AgentEvent::Text("Halfway there.".to_string()),
            AgentEvent::Limited(limit.clone()),
        ]
        .into_iter()
        .collect();
        result.conclude();

        assert!(result.finished);
        assert_eq!(result.failure.as_deref(), Some(limit.message.as_str()));
        assert_eq!(result.limit, Some(limit));
    }

    #[test]
    fn only_a_limit_that_is_the_first_failure_is_kept() {
        let failed_first: StdoutParseResult = [
            AgentEvent::Failed("Invalid API key provided".to_string()),
            AgentEvent::Limited(Limit::new("You have hit your usage limit.")),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            failed_first.failure.as_deref(),
            Some("Invalid API key provided")
        );
        assert!(failed_first.limit.is_none(), "{failed_first:?}");

        let limited_first: StdoutParseResult = [
            AgentEvent::Limited(Limit::new("You have hit your usage limit.")),
            AgentEvent::Limited(Limit::new("You have hit your weekly limit.")),
            AgentEvent::Failed("turn failed".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            limited_first.failure.as_deref(),
            Some("You have hit your usage limit.")
        );
        assert_eq!(
            limited_first.limit.map(|limit| limit.message),
            Some("You have hit your usage limit.".to_string())
        );
    }

    #[test]
    fn each_window_keeps_its_latest_report_in_the_order_first_reported() {
        let result: StdoutParseResult = [
            AgentEvent::Window(Window::new("five_hour").with_used_percent(40.0)),
            AgentEvent::Window(Window::new("seven_day").with_used_percent(10.0)),
            AgentEvent::Window(Window::new("five_hour").with_used_percent(43.0)),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            result.windows,
            [
                Window::new("five_hour").with_used_percent(43.0),
                Window::new("seven_day").with_used_percent(10.0),
            ]
        );
        assert!(!result.finished);
        assert!(result.failure.is_none());
    }

    #[test]
    fn the_last_occupancy_reported_wins_and_ends_nothing() {
        let result: StdoutParseResult = [
            AgentEvent::Occupancy(Occupancy::reported(258_400, Some(1_200)).expect("a window")),
            AgentEvent::finished(Some("completed".to_string())),
            AgentEvent::Occupancy(Occupancy::reported(475_000, Some(24_638)).expect("a window")),
        ]
        .into_iter()
        .collect();

        assert_eq!(result.occupancy, Occupancy::reported(475_000, Some(24_638)));
        assert!(result.finished);
        assert_eq!(result.finish_reason.as_deref(), Some("completed"));
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
