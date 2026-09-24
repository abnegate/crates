use abnegate_llm::GeneratedImage;
use abnegate_llm::Message;
use abnegate_llm::Role;
use abnegate_llm::ToolCall;
use serde::Deserialize;
use serde::Serialize;

/// The storage format version a [`ReplayMessage`] is written in.
pub const VERSION: u16 = 1;

/// A message as the conversation store keeps it: independent of the
/// provider's asymmetric wire format, and able to round-trip images.
///
/// Built from the [`Message`] it stores with [`From`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReplayMessage {
    pub version: u16,
    pub role: Role,
    pub content: Option<String>,
    pub name: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
    pub images: Vec<String>,
    pub generated_images: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking_blocks: Vec<serde_json::Value>,
}

impl From<&Message> for ReplayMessage {
    fn from(message: &Message) -> Self {
        Self {
            version: VERSION,
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
        }
    }
}

impl ReplayMessage {
    pub fn into_message(self) -> Message {
        let mut message = Message::new(self.role);
        message.content = self.content;
        message.name = self.name;
        message.tool_calls = self.tool_calls;
        message.tool_call_id = self.tool_call_id;
        message.images = self.images;
        message.generated_images = self
            .generated_images
            .into_iter()
            .map(GeneratedImage::new)
            .collect();
        message.reasoning_content = self.reasoning_content;
        message.thinking_blocks = self.thinking_blocks;
        message
    }
}
