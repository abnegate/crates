use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ResponseFormat {
    Text,
    /// A JSON answer, shaped by `schema` when there is one.
    ///
    /// `strict` asks OpenAI to enforce the schema exactly. It refuses any
    /// schema in which an object leaves out `additionalProperties: false` or
    /// a property from `required`, so it is off unless a caller has written
    /// the schema for it.
    Json {
        schema: Option<serde_json::Value>,
        #[serde(default)]
        strict: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_keeps_its_schema_through_a_round_trip() {
        let format = ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: true,
        };

        let json = serde_json::to_string(&format).unwrap();
        let roundtrip: ResponseFormat = serde_json::from_str(&json).unwrap();

        match roundtrip {
            ResponseFormat::Json { schema, strict } => {
                assert!(schema.is_some());
                assert!(strict);
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn a_format_written_before_strict_existed_is_not_strict() {
        let format: ResponseFormat =
            serde_json::from_str(r#"{"Json":{"schema":{"type":"object"}}}"#).unwrap();

        assert!(matches!(format, ResponseFormat::Json { strict: false, .. }));
    }

    #[test]
    fn text_round_trips() {
        let json = serde_json::to_string(&ResponseFormat::Text).unwrap();
        let roundtrip: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert!(matches!(roundtrip, ResponseFormat::Text));
    }
}
