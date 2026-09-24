use serde::{Deserialize, Serialize};

use crate::modality::ResponseFormat;

const DEFAULT_TEMPERATURE: f64 = 0.7;
const DEFAULT_MAXIMUM_TOKENS: u32 = 4096;

/// One prompt for a [`TextProvider`](crate::TextProvider): a system prompt, a
/// user prompt, and how the answer should come back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    pub temperature: f64,
    /// The most tokens the answer may use. Serialised as `max_tokens`.
    #[serde(rename = "max_tokens")]
    pub maximum_tokens: u32,
    pub response_format: Option<ResponseFormat>,
    pub context: Option<serde_json::Value>,
}

impl TextRequest {
    /// Ask `user_prompt` under `system_prompt` for up to 4,096 tokens of prose
    /// at temperature 0.7.
    pub fn new(system_prompt: impl Into<String>, user_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            user_prompt: user_prompt.into(),
            temperature: DEFAULT_TEMPERATURE,
            maximum_tokens: DEFAULT_MAXIMUM_TOKENS,
            response_format: None,
            context: None,
        }
    }

    /// Set [`Self::temperature`].
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set [`Self::maximum_tokens`].
    pub fn with_maximum_tokens(mut self, maximum_tokens: u32) -> Self {
        self.maximum_tokens = maximum_tokens;
        self
    }

    /// Ask for the answer in `response_format` rather than as prose.
    pub fn with_response_format(mut self, response_format: ResponseFormat) -> Self {
        self.response_format = Some(response_format);
        self
    }

    /// Carry `context` alongside the prompts.
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_in_the_defaults() {
        let request = TextRequest::new("system", "user");
        assert_eq!(request.system_prompt, "system");
        assert_eq!(request.user_prompt, "user");
        assert!((request.temperature - 0.7).abs() < f64::EPSILON);
        assert_eq!(request.maximum_tokens, 4096);
        assert!(request.response_format.is_none());
        assert!(request.context.is_none());
    }

    #[test]
    fn the_answer_limit_keeps_its_serialised_name() {
        let body = serde_json::to_value(TextRequest::new("system", "user")).unwrap();
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("maximum_tokens").is_none(), "{body}");

        let saved = r#"{"system_prompt":"s","user_prompt":"u","temperature":0.2,"max_tokens":512,"response_format":null,"context":null}"#;
        let request: TextRequest = serde_json::from_str(saved).unwrap();
        assert_eq!(request.maximum_tokens, 512);
    }

    #[test]
    fn round_trips_through_json() {
        let request = TextRequest::new("System prompt here", "User prompt here")
            .with_temperature(0.3)
            .with_maximum_tokens(2000)
            .with_response_format(ResponseFormat::Json {
                schema: None,
                strict: false,
            })
            .with_context(serde_json::json!({ "key": "value" }));

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: TextRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.system_prompt, "System prompt here");
        assert_eq!(roundtrip.user_prompt, "User prompt here");
        assert!((roundtrip.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(roundtrip.maximum_tokens, 2000);
        assert!(roundtrip.response_format.is_some());
        assert_eq!(
            roundtrip.context,
            Some(serde_json::json!({ "key": "value" }))
        );
    }
}
