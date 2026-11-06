//! What one of an agent's own tools printed.

use abnegate_llm::ToolCall;

const OMITTED: &str = "bytes left out";

/// What one of the agent's own tool calls printed once it finished, and
/// whether it worked: the text the agent read back, which it never streams
/// as prose.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Output {
    /// The call that printed it, its arguments as the agent gave them and
    /// not scrubbed, as in [`AgentEvent::Tool`](crate::AgentEvent::Tool).
    pub call: ToolCall,
    /// What it printed, scrubbed of the run's secrets, with no more of it
    /// kept than the run's
    /// [`tool_output_limit`](crate::CliSettings::tool_output_limit) allows.
    pub text: String,
    /// Whether the call worked: a Claude tool result not marked an error,
    /// or a Codex command that exited zero.
    pub succeeded: bool,
}

impl Output {
    /// What `call` printed, `text`, and whether it `succeeded`.
    pub fn new(call: ToolCall, text: impl Into<String>, succeeded: bool) -> Self {
        Self {
            call,
            text: text.into(),
            succeeded,
        }
    }

    /// The same output, printing `text` instead.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// The same output, keeping no more than `limit` bytes of its text: a
    /// longer text keeps its start and its end, each at most half of `limit`
    /// and cut on a character boundary, around a line saying how many bytes
    /// were left out.
    pub(crate) fn truncated(mut self, limit: usize) -> Self {
        let length = self.text.len();
        if length <= limit {
            return self;
        }
        let head = self.text.floor_char_boundary(limit / 2);
        let tail = self.text.ceil_char_boundary(length - (limit - limit / 2));
        self.text = format!(
            "{}\n[{} {OMITTED}]\n{}",
            &self.text[..head],
            tail - head,
            &self.text[tail..]
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use abnegate_llm::ToolCall;

    use super::Output;

    #[test]
    fn an_output_keeps_its_call_text_and_outcome() {
        let call = ToolCall::function("toolu_01", "Bash", r#"{"command":"ls"}"#);

        let output = Output::new(call, "a\nb", false).with_text("c");

        assert_eq!(output.call.id, "toolu_01");
        assert_eq!(output.call.function.name, "Bash");
        assert_eq!(output.text, "c");
        assert!(!output.succeeded);
    }

    #[test]
    fn text_within_the_limit_is_left_whole() {
        let call = ToolCall::function("toolu_01", "Bash", "{}");

        let output = Output::new(call, "0123456789", true).truncated(10);

        assert_eq!(output.text, "0123456789");
    }

    #[test]
    fn longer_text_keeps_its_start_and_end_around_what_was_left_out() {
        let call = ToolCall::function("toolu_01", "Bash", "{}");
        let text = format!("{}https://example.com/pull/7", "x".repeat(1000));

        let output = Output::new(call, text, true).truncated(52);

        assert_eq!(
            output.text,
            format!(
                "{}\n[974 bytes left out]\nhttps://example.com/pull/7",
                "x".repeat(26)
            )
        );
    }

    #[test]
    fn longer_text_is_cut_on_character_boundaries() {
        let call = ToolCall::function("toolu_01", "Bash", "{}");
        let text = format!("a{}", "é".repeat(100));

        let output = Output::new(call, text, true).truncated(13);

        assert_eq!(output.text, "aéé\n[190 bytes left out]\nééé");
    }
}
