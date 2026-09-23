//! Persistence shape for a stored conversation.
//!
//! The [`Message`] serializer expands images into content parts, the shape a
//! vision model accepts, and cannot round-trip. A stored history uses these two
//! functions instead, through `#[serde(with = "abnegate_llm::history")]`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::GeneratedImage;
use crate::wire::ImageUrl;
use crate::wire::Message;
use crate::wire::Role;
use crate::wire::ToolCall;

#[derive(Serialize, Deserialize)]
struct Record {
    role: Role,
    content: Option<String>,
    name: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
    tool_call_id: Option<String>,
    #[serde(default)]
    images: Vec<String>,
    #[serde(default)]
    generated_images: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    thinking_blocks: Vec<serde_json::Value>,
}

pub fn serialize<S: Serializer>(messages: &[Message], serializer: S) -> Result<S::Ok, S::Error> {
    messages
        .iter()
        .map(|message| Record {
            role: message.role,
            content: message.content.clone(),
            name: message.name.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            images: message.images.clone(),
            generated_images: message
                .generated_images
                .iter()
                .map(|image| image.image_url.url.clone())
                .collect(),
            reasoning_content: message.reasoning_content.clone(),
            thinking_blocks: message.thinking_blocks.clone(),
        })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Message>, D::Error> {
    Ok(Vec::<Record>::deserialize(deserializer)?
        .into_iter()
        .map(|record| Message {
            role: record.role,
            content: record.content,
            name: record.name,
            tool_calls: record.tool_calls,
            tool_call_id: record.tool_call_id,
            images: record.images,
            generated_images: record
                .generated_images
                .into_iter()
                .map(|url| GeneratedImage {
                    image_url: ImageUrl { url },
                })
                .collect(),
            reasoning_content: record.reasoning_content,
            thinking_blocks: record.thinking_blocks,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::wire::Message;
    use crate::wire::Role;

    #[derive(Serialize, Deserialize)]
    struct Stored {
        #[serde(with = "crate::history")]
        messages: Vec<Message>,
    }

    #[test]
    fn an_uploaded_image_survives_a_round_trip_the_wire_shape_would_lose() {
        let mut message = Message::user("what is this");
        message.images = vec!["data:image/png;base64,abc".to_string()];

        let json = serde_json::to_string(&Stored {
            messages: vec![message],
        })
        .unwrap();
        let stored: Stored = serde_json::from_str(&json).unwrap();

        assert_eq!(stored.messages[0].role, Role::User);
        assert_eq!(stored.messages[0].content.as_deref(), Some("what is this"));
        assert_eq!(stored.messages[0].images, ["data:image/png;base64,abc"]);
    }

    #[test]
    fn reasoning_and_thinking_blocks_survive_a_round_trip() {
        let mut message = Message::assistant("Paris.");
        message.reasoning_content = Some("The capital is Paris.".to_string());
        message.thinking_blocks = vec![serde_json::json!({"type": "thinking", "signature": "sig"})];

        let json = serde_json::to_string(&Stored {
            messages: vec![message],
        })
        .unwrap();
        let stored: Stored = serde_json::from_str(&json).unwrap();

        assert_eq!(
            stored.messages[0].reasoning_content.as_deref(),
            Some("The capital is Paris.")
        );
        assert_eq!(stored.messages[0].thinking_blocks[0]["signature"], "sig");
    }

    #[test]
    fn a_generated_image_is_stored_as_its_url_and_read_back_as_one() {
        let json = r#"{"messages":[{"role":"assistant","content":"here","generated_images":["data:image/png;base64,abc"]}]}"#;

        let stored: Stored = serde_json::from_str(json).unwrap();

        assert_eq!(
            stored.messages[0].generated_images[0].image_url.url,
            "data:image/png;base64,abc"
        );
        assert!(
            serde_json::to_string(&stored)
                .unwrap()
                .contains(r#""generated_images":["data:image/png;base64,abc"]"#)
        );
    }

    #[test]
    fn an_empty_history_round_trips() {
        let json = serde_json::to_string(&Stored {
            messages: Vec::new(),
        })
        .unwrap();
        let stored: Stored = serde_json::from_str(&json).unwrap();

        assert!(stored.messages.is_empty());
    }
}
