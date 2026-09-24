use serde::Serialize;
use serde::Serializer;

use crate::modality::ResponseFormat;
use crate::wire::message::Message;
use crate::wire::tool_choice::ToolChoice;
use crate::wire::tool_definition::ToolDefinition;

const SCHEMA_NAME: &str = "response";

/// A chat completion request.
///
/// Borrowed because the agent loop already owns the conversation and the tool
/// definitions, and a request that took them by value would clone the whole
/// history once per turn.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<&'a [ToolDefinition]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// The most tokens the answer may use. Sent as `max_tokens`.
    #[serde(rename = "max_tokens", skip_serializing_if = "Option::is_none")]
    pub maximum_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<&'a [String]>,
    /// Sent in the OpenAI shape: `json_schema` with the schema when there is
    /// one, strict only when asked, `json_object` when there is not, and
    /// `text` for prose.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "response_format"
    )]
    pub response_format: Option<&'a ResponseFormat>,
}

fn response_format<S: Serializer>(
    format: &Option<&ResponseFormat>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let body = match format {
        Some(ResponseFormat::Json {
            schema: Some(schema),
            strict,
        }) => {
            let mut format = serde_json::json!({ "name": SCHEMA_NAME, "schema": schema });
            if *strict {
                format["strict"] = true.into();
            }
            serde_json::json!({ "type": "json_schema", "json_schema": format })
        }
        Some(ResponseFormat::Json { schema: None, .. }) => {
            serde_json::json!({ "type": "json_object" })
        }
        Some(ResponseFormat::Text) | None => serde_json::json!({ "type": "text" }),
    };
    body.serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::ChatRequest;
    use crate::modality::ResponseFormat;
    use crate::wire::message::Message;
    use crate::wire::tool_choice::ToolChoice;
    use crate::wire::tool_definition::ToolDefinition;

    #[test]
    fn a_response_format_is_sent_in_the_openai_shape() {
        let messages = [Message::user("Hello")];
        let schema = ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        };
        let object = ResponseFormat::Json {
            schema: None,
            strict: false,
        };
        let body = |format| {
            serde_json::to_value(ChatRequest {
                model: "gpt-4",
                messages: &messages,
                tools: None,
                tool_choice: None,
                temperature: None,
                maximum_tokens: None,
                stream: None,
                stop: None,
                response_format: format,
            })
            .unwrap()
        };

        let strict = ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: true,
        };
        let structured = body(Some(&schema));
        assert_eq!(structured["response_format"]["type"], "json_schema");
        assert_eq!(
            structured["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );
        assert!(
            structured["response_format"]["json_schema"]
                .get("strict")
                .is_none()
        );
        assert_eq!(
            body(Some(&strict))["response_format"]["json_schema"]["strict"],
            true
        );
        assert_eq!(
            body(Some(&object))["response_format"]["type"],
            "json_object"
        );
        assert_eq!(
            body(Some(&ResponseFormat::Text))["response_format"]["type"],
            "text"
        );
        assert!(body(None).get("response_format").is_none());
    }

    #[test]
    fn a_request_carries_its_model_body_and_sampling() {
        let messages = [Message::user("Hello")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: Some(0.7),
            maximum_tokens: Some(1000),
            stream: Some(false),
            stop: None,
            response_format: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4"));
        assert!(json.contains("Hello"));
        assert!(json.contains("0.7"));
    }

    #[test]
    fn the_answer_limit_is_sent_as_max_tokens() {
        let messages = [Message::user("Hello")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            maximum_tokens: Some(1000),
            stream: None,
            stop: None,
            response_format: None,
        };

        let body = serde_json::to_value(&request).unwrap();

        assert_eq!(body["max_tokens"], 1000);
        assert!(body.get("maximum_tokens").is_none(), "{body}");
    }

    #[test]
    fn unset_options_are_omitted_rather_than_sent_as_null() {
        let messages = [Message::user("Hello")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            maximum_tokens: None,
            stream: None,
            stop: None,
            response_format: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4"));
        assert!(json.contains("Hello"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("stream"));
    }

    #[test]
    fn tools_and_a_tool_choice_reach_the_body() {
        let tools = [ToolDefinition::function(
            "test_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        )];
        let messages = [Message::user("Use the tool")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: Some(&tools),
            tool_choice: Some(ToolChoice::auto()),
            temperature: Some(0.5),
            maximum_tokens: Some(2048),
            stream: Some(false),
            stop: None,
            response_format: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("A test tool"));
        assert!(json.contains("auto"));
    }

    #[test]
    fn every_turn_of_the_conversation_reaches_the_body() {
        let messages = [
            Message::system("You are helpful"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
            Message::user("How are you?"),
        ];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            maximum_tokens: None,
            stream: None,
            stop: None,
            response_format: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("system"));
        assert!(json.contains("user"));
        assert!(json.contains("assistant"));
    }
}
