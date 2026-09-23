use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ResponseFormat {
    Text,
    Json { schema: Option<serde_json::Value> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_keeps_its_schema_through_a_round_trip() {
        let format = ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
        };

        let json = serde_json::to_string(&format).unwrap();
        let roundtrip: ResponseFormat = serde_json::from_str(&json).unwrap();

        match roundtrip {
            ResponseFormat::Json { schema } => assert!(schema.is_some()),
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn text_round_trips() {
        let json = serde_json::to_string(&ResponseFormat::Text).unwrap();
        let roundtrip: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, ResponseFormat::Text));
    }
}
