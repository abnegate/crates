use serde::{Deserialize, Serialize};

use crate::wire::image_url::ImageUrl;

/// One part of a multimodal message body.
///
/// A variant may gain a field in a minor release, so a value is built with
/// [`ContentPart::text`] or [`ContentPart::image_url`] and a pattern outside
/// this crate ends in `..`:
///
/// ```compile_fail,E0639
/// let part = abnegate_llm::ContentPart::Text {
///     text: "Describe this image.".to_string(),
/// };
/// # let _ = part;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentPart {
    /// Prose.
    #[non_exhaustive]
    Text { text: String },
    /// An image, by URL or data URL.
    #[non_exhaustive]
    ImageUrl { image_url: ImageUrl },
}

impl ContentPart {
    /// The prose `text`.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// The image at `url`, which may be a data URL.
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl {
            image_url: ImageUrl::new(url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContentPart;

    #[test]
    fn each_part_keeps_its_wire_shape() {
        assert_eq!(
            serde_json::to_value(ContentPart::text("hello")).unwrap(),
            serde_json::json!({ "type": "text", "text": "hello" })
        );
        assert_eq!(
            serde_json::to_value(ContentPart::image_url("data:image/png;base64,AAAA")).unwrap(),
            serde_json::json!({
                "type": "image_url",
                "image_url": { "url": "data:image/png;base64,AAAA" }
            })
        );
    }
}
