use serde::{Deserialize, Serialize};

use crate::modality::ResponseFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fn new(system_prompt: impl Into<String>, user_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            user_prompt: user_prompt.into(),
            temperature: 0.7,
            maximum_tokens: 4096,
            response_format: None,
            context: None,
        }
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
        let request = TextRequest {
            system_prompt: "System prompt here".into(),
            user_prompt: "User prompt here".into(),
            temperature: 0.3,
            maximum_tokens: 2000,
            response_format: Some(ResponseFormat::Json {
                schema: None,
                strict: false,
            }),
            context: Some(serde_json::json!({ "key": "value" })),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: TextRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.system_prompt, "System prompt here");
        assert_eq!(roundtrip.user_prompt, "User prompt here");
        assert!((roundtrip.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(roundtrip.maximum_tokens, 2000);
        assert!(roundtrip.response_format.is_some());
        assert!(roundtrip.context.is_some());
    }
}
