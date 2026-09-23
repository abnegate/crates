use serde::{Deserialize, Serialize};

/// An image reference.
///
/// Data URLs are accepted by OpenAI-compatible providers, which is how an
/// uploaded image reaches a vision model without object storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}
