use crate::wire::Message;
use crate::wire::Usage;

/// One completion's result, normalised across provider kinds.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Completion {
    /// The provider that actually produced this, which under a fallback chain
    /// is not necessarily the one the caller configured first.
    pub provider: String,
    /// The answer, as an assistant message.
    pub message: Message,
    /// The tokens it took, when the provider said.
    pub usage: Option<Usage>,
    /// Why the model stopped, in the provider's own words, when it said.
    pub finish_reason: Option<String>,
}

impl Completion {
    /// `message` as `provider` answered it, with no usage or finish reason.
    pub fn new(provider: impl Into<String>, message: Message) -> Self {
        Self {
            provider: provider.into(),
            message,
            usage: None,
            finish_reason: None,
        }
    }

    /// Set [`Self::usage`].
    pub fn with_usage(mut self, usage: impl Into<Option<Usage>>) -> Self {
        self.usage = usage.into();
        self
    }

    /// Set [`Self::finish_reason`].
    pub fn with_finish_reason(mut self, finish_reason: impl Into<Option<String>>) -> Self {
        self.finish_reason = finish_reason.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_carries_what_it_was_given() {
        let completion = Completion::new("gateway", Message::assistant("hi"))
            .with_usage(Usage::new(3, 4))
            .with_finish_reason("stop".to_string());

        assert_eq!(completion.provider, "gateway");
        assert_eq!(completion.message.content.as_deref(), Some("hi"));
        assert_eq!(completion.usage.map(|usage| usage.total_tokens), Some(7));
        assert_eq!(completion.finish_reason.as_deref(), Some("stop"));
    }
}
