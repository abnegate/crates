use crate::event::AgentEvent;
use crate::kind::AgentKind;
use crate::parser::claude;
use crate::parser::codex;

/// One turn of an agent's output, read a line at a time, so a line whose
/// meaning hangs on the lines before it is read beside them.
///
/// A run reads its lines through one, from [`AgentKind::turn`].
#[derive(Debug, Clone)]
pub struct Turn {
    agent: AgentKind,
    claude: claude::Turn,
}

impl Turn {
    pub(crate) fn new(agent: AgentKind) -> Self {
        Self {
            agent,
            claude: claude::Turn::default(),
        }
    }

    /// Translate one output line, appending whatever it means.
    ///
    /// A line this agent has no opinion about appends nothing rather than
    /// failing: agents add event types between releases, and a stream that
    /// aborted on the first unrecognised line would lose the whole answer.
    pub fn interpret(&mut self, line: &str, events: &mut Vec<AgentEvent>) {
        match self.agent {
            AgentKind::Claude => self.claude.interpret(line, events),
            AgentKind::Codex => codex::interpret(line, events),
        }
    }

    /// Whether work the agent sent to the background is running, or a wakeup
    /// it set has yet to fire, as of the last line that said so.
    pub fn background_running(&self) -> bool {
        match self.agent {
            AgentKind::Claude => self.claude.background_running(),
            AgentKind::Codex => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::event::AgentEvent;
    use crate::kind::AgentKind;

    #[test]
    fn a_claude_turn_remembers_a_refused_window_for_the_line_that_answers_it() {
        let mut turn = AgentKind::Claude.turn();
        let mut events = Vec::new();

        turn.interpret(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}"#,
            &mut events,
        );
        turn.interpret(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"You've hit your limit"}]},"parent_tool_use_id":null,"is_api_error_message":true}"#,
            &mut events,
        );

        let limits: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Limited(limit) => Some(limit.message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            limits,
            ["rate limit reached (five_hour, rejected): You've hit your limit"]
        );
    }

    #[test]
    fn a_claude_turn_follows_what_it_says_runs_in_the_background() {
        const RUNNING: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"build"}]}"#;
        const NOTHING: &str =
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#;
        const UNREADABLE: &str =
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":"soon"}"#;
        const STARTED: &str =
            r#"{"type":"system","subtype":"task_started","task_id":"b2","task_type":"local_bash"}"#;
        let mut turn = AgentKind::Claude.turn();
        let mut events = Vec::new();

        assert!(!turn.background_running());
        turn.interpret(RUNNING, &mut events);
        assert!(turn.background_running());
        turn.interpret(UNREADABLE, &mut events);
        assert!(
            turn.background_running(),
            "a notice in a new shape says nothing of what runs"
        );
        turn.interpret(NOTHING, &mut events);
        assert!(!turn.background_running());
        turn.interpret(STARTED, &mut events);
        assert!(
            !turn.background_running(),
            "only the notice that lists what runs says it runs"
        );
    }

    const WAKE: &str = r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_wake","name":"ScheduleWakeup","input":{"delaySeconds":60,"reason":"wait"}}]}}"#;

    #[test]
    fn a_claude_turn_waits_for_a_wakeup_it_set_until_it_fires() {
        const FIRED: &str =
            r#"{"type":"command_lifecycle","state":"started","wakeup_source":"loop_wakeup"}"#;
        const OTHER_COMMAND: &str = r#"{"type":"command_lifecycle","state":"started"}"#;
        const COMPLETED: &str = r#"{"type":"command_lifecycle","state":"completed"}"#;
        let mut turn = AgentKind::Claude.turn();
        let mut events = Vec::new();

        turn.interpret(WAKE, &mut events);
        assert!(turn.background_running());
        for line in [OTHER_COMMAND, COMPLETED] {
            turn.interpret(line, &mut events);
            assert!(turn.background_running(), "{line}");
        }
        turn.interpret(FIRED, &mut events);
        assert!(!turn.background_running());
    }

    #[test]
    fn a_claude_turn_waits_for_no_wakeup_refused_cancelled_or_set_by_a_subagent() {
        const REFUSED: &str = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_wake","content":"no","is_error":true}]}}"#;
        const CANCEL: &str = r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_stop","name":"ScheduleWakeup","input":{"stop":true}}]}}"#;
        const SUBAGENTS: &str = r#"{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"tool_use","id":"toolu_its","name":"ScheduleWakeup","input":{"delaySeconds":60}}]}}"#;

        for (set, undone) in [
            (Some(WAKE), REFUSED),
            (Some(WAKE), CANCEL),
            (None, SUBAGENTS),
        ] {
            let mut turn = AgentKind::Claude.turn();
            let mut events = Vec::new();
            for line in set.into_iter().chain([undone]) {
                turn.interpret(line, &mut events);
            }
            assert!(!turn.background_running(), "{undone}");
        }
    }

    #[test]
    fn a_codex_turn_has_nothing_in_the_background() {
        let mut turn = AgentKind::Codex.turn();
        let mut events = Vec::new();

        turn.interpret(
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}"#,
            &mut events,
        );

        assert!(!turn.background_running());
    }

    #[test]
    fn a_codex_turn_reads_codexs_lines() {
        let mut turn = AgentKind::Codex.turn();
        let mut events = Vec::new();

        turn.interpret(
            r#"{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":2}}"#,
            &mut events,
        );

        assert!(matches!(events.last(), Some(AgentEvent::Finished { .. })));
    }
}
