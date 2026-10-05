use serde::Deserialize;

use crate::wire::message::Message;

/// One completion choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// Which answer this is, from 0.
    #[serde(default)]
    pub index: u32,
    /// The answer.
    pub message: Message,
    /// Why the model stopped, such as `stop` or `tool_calls`.
    pub finish_reason: Option<String>,
}

impl Choice {
    /// The answer at `index`, saying `message`, with no reason given for
    /// stopping.
    pub fn new(index: u32, message: Message) -> Self {
        Self {
            index,
            message,
            finish_reason: None,
        }
    }

    /// The same choice, stopped for `finish_reason`.
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Self {
        self.finish_reason = Some(finish_reason.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::Choice;
    use crate::wire::message::Message;

    #[test]
    fn a_built_choice_keeps_its_answer_and_reason() {
        let choice = Choice::new(0, Message::assistant("Hello!"));
        assert_eq!(choice.index, 0);
        assert_eq!(choice.message.content.as_deref(), Some("Hello!"));
        assert!(choice.finish_reason.is_none());

        let stopped = choice.with_finish_reason("stop");
        assert_eq!(stopped.finish_reason.as_deref(), Some("stop"));
    }
}
