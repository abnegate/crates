use serde::Deserialize;

use crate::wire::generated_image::GeneratedImage;
use crate::wire::null_to_default;
use crate::wire::role::Role;
use crate::wire::stream_tool_call::StreamToolCall;

/// What one streaming chunk adds to the reply.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StreamDelta {
    pub role: Option<Role>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCall>>,
    #[serde(default, rename = "images", deserialize_with = "null_to_default")]
    pub generated_images: Vec<GeneratedImage>,
    #[serde(default, alias = "reasoning", alias = "thinking")]
    pub reasoning_content: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub thinking_blocks: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::StreamDelta;

    #[test]
    fn a_default_delta_adds_nothing() {
        let delta = StreamDelta::default();
        assert!(delta.role.is_none());
        assert!(delta.content.is_none());
        assert!(delta.tool_calls.is_none());
        assert!(delta.generated_images.is_empty());
        assert!(delta.reasoning_content.is_none());
        assert!(delta.thinking_blocks.is_empty());
    }
}
