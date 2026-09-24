use serde::Deserialize;

/// Function call details.
///
/// `arguments` is stored as the model emitted it. Serialisation always emits a
/// single JSON value: an Ollama adapter `json.loads`s the whole string and
/// returns 500 on concatenated documents (`{...}{...}`).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

impl FunctionCall {
    /// A call to `name` with `arguments`, a JSON object as a string.
    pub fn new(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
        }
    }
}

impl serde::Serialize for FunctionCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("FunctionCall", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("arguments", &wire_arguments(&self.arguments))?;
        state.end()
    }
}

/// First JSON value in an OpenAI-style tool `arguments` string.
///
/// Empty, `null`, or unparseable input becomes `{}`. Extra documents after the
/// first value are dropped so a later Ollama round-trip cannot fail to parse.
pub fn wire_arguments(arguments: &str) -> String {
    let remaining = arguments.trim();
    if remaining.is_empty() {
        return "{}".to_string();
    }
    let mut stream = serde_json::Deserializer::from_str(remaining).into_iter::<serde_json::Value>();
    match stream.next() {
        Some(Ok(serde_json::Value::Null)) => "{}".to_string(),
        Some(Ok(serde_json::Value::Object(_))) => {
            remaining[..stream.byte_offset()].trim().to_string()
        }
        Some(Ok(value)) => value.to_string(),
        _ => "{}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::wire_arguments;

    #[test]
    fn wire_arguments_keeps_a_single_object() {
        let arguments = r#"{"query":"documents reminders console"}"#;
        assert_eq!(wire_arguments(arguments), arguments);
    }

    #[test]
    fn wire_arguments_drops_extra_json_documents() {
        let arguments = r#"{"query":"documents reminders console"}{"limit":5}"#;
        assert_eq!(
            wire_arguments(arguments),
            r#"{"query":"documents reminders console"}"#
        );
        let parsed: serde_json::Value = serde_json::from_str(&wire_arguments(arguments)).unwrap();
        assert!(parsed.is_object());
    }

    #[test]
    fn wire_arguments_normalises_empty_and_null() {
        assert_eq!(wire_arguments(""), "{}");
        assert_eq!(wire_arguments("   "), "{}");
        assert_eq!(wire_arguments("null"), "{}");
        assert_eq!(wire_arguments("{"), "{}");
    }
}
