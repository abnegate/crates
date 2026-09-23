use serde::{Deserialize, Serialize};

use crate::wire::image_url::ImageUrl;

/// One part of a multimodal message body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}
