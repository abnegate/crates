use serde::{Deserialize, Serialize};

/// The shape a model's answer is asked to take.
///
/// [`ResponseFormat::Json`] may gain a field in a minor release, so it is
/// built with [`ResponseFormat::json`], [`ResponseFormat::json_schema`] or
/// [`ResponseFormat::strict_json_schema`], and a pattern outside this crate
/// ends in `..`:
///
/// ```compile_fail,E0639
/// let format = abnegate_llm::ResponseFormat::Json {
///     schema: None,
///     strict: false,
/// };
/// # let _ = format;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ResponseFormat {
    /// Prose.
    Text,
    /// A JSON answer, shaped by `schema` when there is one.
    ///
    /// `strict` asks OpenAI to enforce the schema exactly. It refuses any
    /// schema in which an object leaves out `additionalProperties: false` or
    /// a property from `required`, so it is off unless a caller has written
    /// the schema for it.
    #[non_exhaustive]
    Json {
        schema: Option<serde_json::Value>,
        #[serde(default)]
        strict: bool,
    },
}

impl ResponseFormat {
    /// A JSON answer of any shape.
    pub fn json() -> Self {
        Self::Json {
            schema: None,
            strict: false,
        }
    }

    /// A JSON answer shaped by the JSON schema `schema`, which the provider
    /// may treat as guidance.
    pub fn json_schema(schema: serde_json::Value) -> Self {
        Self::Json {
            schema: Some(schema),
            strict: false,
        }
    }

    /// A JSON answer the provider holds to `schema` exactly, for a schema
    /// written for OpenAI's strict mode.
    pub fn strict_json_schema(schema: serde_json::Value) -> Self {
        Self::Json {
            schema: Some(schema),
            strict: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_json_constructor_keeps_its_wire_shape() {
        let schema = serde_json::json!({ "type": "object" });
        for (format, wire) in [
            (
                ResponseFormat::json(),
                serde_json::json!({ "Json": { "schema": null, "strict": false } }),
            ),
            (
                ResponseFormat::json_schema(schema.clone()),
                serde_json::json!({ "Json": { "schema": schema, "strict": false } }),
            ),
            (
                ResponseFormat::strict_json_schema(schema.clone()),
                serde_json::json!({ "Json": { "schema": schema, "strict": true } }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&format).unwrap(), wire);
        }
    }

    #[test]
    fn json_keeps_its_schema_through_a_round_trip() {
        let format = ResponseFormat::strict_json_schema(serde_json::json!({ "type": "object" }));

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
