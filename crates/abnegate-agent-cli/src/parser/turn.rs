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

        let failures: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Failed(message) => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            failures,
            [
                r#"rate limit reached: {"status":"rejected","rateLimitType":"five_hour"}: You've hit your limit"#
            ]
        );
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
