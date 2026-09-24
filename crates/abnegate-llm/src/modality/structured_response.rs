use serde::{Deserialize, Serialize};

use crate::modality::TextResponse;
use crate::provider::ProviderError;

/// A value of the shape a structured request asked for, and what it cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StructuredResponse {
    pub value: serde_json::Value,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl StructuredResponse {
    /// `value` as `model` answered it, with no tokens counted.
    pub fn new(value: serde_json::Value, model: impl Into<String>) -> Self {
        Self {
            value,
            model: model.into(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// Set how many tokens the prompt and the answer took.
    pub fn with_tokens(mut self, input_tokens: u32, output_tokens: u32) -> Self {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self
    }

    /// Read the value out of a text answer that should be JSON, keeping the
    /// answer's model and token counts.
    pub fn from_text(response: TextResponse) -> Result<Self, ProviderError> {
        let value = serde_json::from_str(&response.content).map_err(|error| {
            ProviderError::parse(format!("failed to parse structured output: {error}"))
        })?;
        Ok(Self::new(value, response.model)
            .with_tokens(response.input_tokens, response.output_tokens))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(content: &str) -> TextResponse {
        TextResponse::new(content, "model").with_tokens(7, 3)
    }

    #[test]
    fn a_json_answer_keeps_its_model_and_counts() {
        let structured = StructuredResponse::from_text(answer(r#"{"beats":3}"#)).unwrap();

        assert_eq!(structured.value["beats"], 3);
        assert_eq!(structured.model, "model");
        assert_eq!(structured.input_tokens, 7);
        assert_eq!(structured.output_tokens, 3);
    }

    #[test]
    fn a_prose_answer_is_a_parse_error() {
        let error = StructuredResponse::from_text(answer("not json")).unwrap_err();
        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }
}
