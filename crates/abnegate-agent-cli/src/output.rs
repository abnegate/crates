//! What one of an agent's own tools printed.

use abnegate_llm::ToolCall;

/// What one of the agent's own tool calls printed once it finished, and
/// whether it worked: the text the agent read back, which it never streams
/// as prose.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Output {
    /// The call that printed it.
    pub call: ToolCall,
    /// What it printed, scrubbed of the run's secrets.
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
}
