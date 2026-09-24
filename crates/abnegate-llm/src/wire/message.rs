use serde::{Deserialize, Serialize};

use crate::wire::content_part::ContentPart;
use crate::wire::generated_image::GeneratedImage;
use crate::wire::image_url::ImageUrl;
use crate::wire::null_to_default;
use crate::wire::role::Role;
use crate::wire::tool_call::ToolCall;

/// A chat message.
///
/// `content` stays a plain string for every caller. `images` is serialised by
/// widening the body into OpenAI content parts, the only shape vision models
/// accept; a message with no images serialises exactly as it would without.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    pub name: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
    #[serde(default, skip_deserializing)]
    pub images: Vec<String>,
    #[serde(default, rename = "images", deserialize_with = "null_to_default")]
    pub generated_images: Vec<GeneratedImage>,
    /// Normalized thinking text. Required on the next turn for some providers.
    #[serde(default, alias = "reasoning", alias = "thinking")]
    pub reasoning_content: Option<String>,
    /// Signed thinking blocks. Must be resent with tool results.
    #[serde(default, deserialize_with = "null_to_default")]
    pub thinking_blocks: Vec<serde_json::Value>,
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;

        if self.images.is_empty() {
            if let Some(content) = &self.content {
                map.serialize_entry("content", content)?;
            }
        } else {
            let mut parts = Vec::with_capacity(self.images.len() + 1);
            if let Some(content) = &self.content
                && !content.is_empty()
            {
                parts.push(ContentPart::Text {
                    text: content.clone(),
                });
            }
            for url in &self.images {
                parts.push(ContentPart::ImageUrl {
                    image_url: ImageUrl::new(url.clone()),
                });
            }
            map.serialize_entry("content", &parts)?;
        }

        if let Some(name) = &self.name {
            map.serialize_entry("name", name)?;
        }
        if let Some(tool_calls) = &self.tool_calls {
            map.serialize_entry("tool_calls", tool_calls)?;
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            map.serialize_entry("tool_call_id", tool_call_id)?;
        }
        if let Some(reasoning) = &self.reasoning_content {
            map.serialize_entry("reasoning_content", reasoning)?;
        }
        if !self.thinking_blocks.is_empty() {
            map.serialize_entry("thinking_blocks", &self.thinking_blocks)?;
        }
        map.end()
    }
}

impl Message {
    /// A message from `role` with nothing in it yet, for a caller that fills
    /// in the fields itself.
    pub fn new(role: Role) -> Self {
        Self {
            role,
            content: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            images: Vec::new(),
            generated_images: Vec::new(),
            reasoning_content: None,
            thinking_blocks: Vec::new(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text(Role::Assistant, content)
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls: Some(tool_calls),
            ..Self::new(Role::Assistant)
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(tool_call_id.into()),
            ..Self::text(Role::Tool, content)
        }
    }

    fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            ..Self::new(role)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Message;
    use crate::wire::role::Role;
    use crate::wire::tool_call::ToolCall;

    #[test]
    fn a_new_message_holds_only_its_role() {
        let message = Message::new(Role::Assistant);

        assert_eq!(message.role, Role::Assistant);
        assert!(message.content.is_none());
        assert!(message.tool_calls.is_none());
        assert!(message.images.is_empty());
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({ "role": "assistant" })
        );
    }

    #[test]
    fn a_system_message_carries_its_content() {
        let message = Message::system("You are a helpful assistant");
        assert_eq!(message.role, Role::System);
        assert_eq!(
            message.content,
            Some("You are a helpful assistant".to_string())
        );
        assert!(message.tool_calls.is_none());
    }

    #[test]
    fn a_user_message_carries_its_content() {
        let message = Message::user("Hello!");
        assert_eq!(message.role, Role::User);
        assert_eq!(message.content, Some("Hello!".to_string()));
    }

    #[test]
    fn an_assistant_message_carries_its_content() {
        let message = Message::assistant("Hi there!");
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content, Some("Hi there!".to_string()));
    }

    #[test]
    fn an_assistant_message_with_tools_has_no_content() {
        let tool_call = ToolCall::function("call_123", "read_file", r#"{"path": "/tmp/test.txt"}"#);
        let message = Message::assistant_with_tools(vec![tool_call]);
        assert_eq!(message.role, Role::Assistant);
        assert!(message.content.is_none());
        assert_eq!(message.tool_calls.unwrap().len(), 1);
    }

    #[test]
    fn a_tool_result_carries_the_call_it_answers() {
        let message = Message::tool_result("call_123", "file contents here");
        assert_eq!(message.role, Role::Tool);
        assert_eq!(message.content, Some("file contents here".to_string()));
        assert_eq!(message.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn a_system_message_round_trips() {
        let message = Message::system("You are a helpful assistant");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.role, Role::System);
        assert_eq!(
            deserialized.content,
            Some("You are a helpful assistant".to_string())
        );
        assert!(deserialized.tool_calls.is_none());
        assert!(deserialized.tool_call_id.is_none());
    }

    #[test]
    fn a_user_message_round_trips() {
        let message = Message::user("Hello, how are you?");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.role, Role::User);
        assert_eq!(
            deserialized.content,
            Some("Hello, how are you?".to_string())
        );
    }

    #[test]
    fn an_assistant_message_round_trips() {
        let message = Message::assistant("I'm doing well, thank you!");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.role, Role::Assistant);
        assert_eq!(
            deserialized.content,
            Some("I'm doing well, thank you!".to_string())
        );
    }

    #[test]
    fn a_tool_result_round_trips() {
        let message = Message::tool_result("call_abc123", "File contents: Hello World");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.role, Role::Tool);
        assert_eq!(
            deserialized.content,
            Some("File contents: Hello World".to_string())
        );
        assert_eq!(deserialized.tool_call_id, Some("call_abc123".to_string()));
    }

    #[test]
    fn tool_calls_round_trip() {
        let tool_call = ToolCall::function(
            "call_xyz789",
            "write_file",
            r#"{"path": "/tmp/file.txt", "content": "Hello"}"#,
        );
        let message = Message::assistant_with_tools(vec![tool_call]);
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.role, Role::Assistant);
        assert!(deserialized.content.is_none());
        let calls = deserialized.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_xyz789");
        assert_eq!(calls[0].function.name, "write_file");
    }

    #[test]
    fn several_tool_calls_round_trip_in_order() {
        let tool_calls = vec![
            ToolCall::function("call_1", "first", "{}"),
            ToolCall::function("call_2", "second", r#"{"param": "value"}"#),
        ];

        let message = Message::assistant_with_tools(tool_calls);
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();

        let calls = deserialized.tool_calls.unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn special_characters_survive_a_round_trip() {
        let message = Message::user("Hello \"world\"! \n\t Special chars: <>&");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.content,
            Some("Hello \"world\"! \n\t Special chars: <>&".to_string())
        );
    }

    #[test]
    fn non_ascii_content_survives_a_round_trip() {
        let message = Message::user("Hello, world! unicode: Rust is good.");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.content,
            Some("Hello, world! unicode: Rust is good.".to_string())
        );
    }

    #[test]
    fn empty_content_survives_a_round_trip() {
        let message = Message::user("");
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content, Some(String::new()));
    }

    #[test]
    fn images_widen_the_body_into_content_parts() {
        let mut message = Message::user("what is this");
        message.images = vec!["data:image/png;base64,abc".to_string()];

        let json = serde_json::to_value(&message).unwrap();
        let parts = json["content"].as_array().expect("content parts");
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "what is this");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,abc");
    }

    #[test]
    fn assistant_reasoning_round_trips_for_later_tool_turns() {
        let mut message = Message::assistant("Paris.");
        message.reasoning_content = Some("The capital is Paris.".into());
        message.thinking_blocks = vec![serde_json::json!({
            "type": "thinking",
            "thinking": "The capital is Paris.",
            "signature": "sig"
        })];
        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.reasoning_content.as_deref(),
            Some("The capital is Paris.")
        );
        assert_eq!(deserialized.thinking_blocks[0]["signature"], "sig");
        let omitted = serde_json::to_string(&Message::assistant("Hi")).unwrap();
        assert!(!omitted.contains("reasoning_content"));
        assert!(!omitted.contains("thinking_blocks"));
    }

    #[test]
    fn an_assistant_message_reads_the_reasoning_alias_and_null_thinking_blocks() {
        let message: Message = serde_json::from_str(
            r#"{"role":"assistant","content":"Paris.","reasoning":"step","thinking_blocks":null}"#,
        )
        .unwrap();
        assert_eq!(message.reasoning_content.as_deref(), Some("step"));
        assert!(message.thinking_blocks.is_empty());
    }
}
