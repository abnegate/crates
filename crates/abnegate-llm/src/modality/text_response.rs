use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = TextResponse {
            content: "Generated text here.".into(),
            model: "claude-opus-5".into(),
            input_tokens: 100,
            output_tokens: 150,
            finish_reason: "stop".into(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: TextResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.content, "Generated text here.");
        assert_eq!(roundtrip.model, "claude-opus-5");
        assert_eq!(roundtrip.input_tokens, 100);
        assert_eq!(roundtrip.output_tokens, 150);
        assert_eq!(roundtrip.finish_reason, "stop");
    }
}
