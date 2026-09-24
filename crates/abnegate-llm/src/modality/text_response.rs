use serde::{Deserialize, Serialize};

const DEFAULT_FINISH_REASON: &str = "stop";

/// What a [`TextProvider`](crate::TextProvider) answered, and what it cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: String,
}

impl TextResponse {
    /// `content` as `model` answered it, with no tokens counted and a finish
    /// reason of `stop`.
    pub fn new(content: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            model: model.into(),
            input_tokens: 0,
            output_tokens: 0,
            finish_reason: DEFAULT_FINISH_REASON.to_string(),
        }
    }

    /// Set how many tokens the prompt and the answer took.
    pub fn with_tokens(mut self, input_tokens: u32, output_tokens: u32) -> Self {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self
    }

    /// Set why the model stopped, in the provider's own words.
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Self {
        self.finish_reason = finish_reason.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_counts_no_tokens_and_stops() {
        let response = TextResponse::new("hello", "qwen3");

        assert_eq!(response.content, "hello");
        assert_eq!(response.model, "qwen3");
        assert_eq!((response.input_tokens, response.output_tokens), (0, 0));
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn round_trips_through_json() {
        let response = TextResponse::new("Generated text here.", "claude-opus-5")
            .with_tokens(100, 150)
            .with_finish_reason("end_turn");

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: TextResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.content, "Generated text here.");
        assert_eq!(roundtrip.model, "claude-opus-5");
        assert_eq!(roundtrip.input_tokens, 100);
        assert_eq!(roundtrip.output_tokens, 150);
        assert_eq!(roundtrip.finish_reason, "end_turn");
    }
}
