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
    /// The storage format it was written in; [`VERSION`] for one built here.
    pub version: u16,
    /// Who wrote the message.
    pub role: Role,
    /// Its text, when it has any.
    pub content: Option<String>,
    /// The participant or tool name the provider was given, if any.
    pub name: Option<String>,
    /// The tool calls an assistant message made.
    pub tool_calls: Option<Vec<ToolCall>>,
    /// The call a tool message answers.
    pub tool_call_id: Option<String>,
    /// Image URLs sent to the model with the message.
    pub images: Vec<String>,
    /// URLs of images the model produced in the message.
    pub generated_images: Vec<String>,
    /// The model's normalized thinking text, which some providers need back
    /// on the next turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Signed thinking blocks, resent verbatim with tool results.
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
    /// The [`Message`] this was stored from, ready to send again.
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
