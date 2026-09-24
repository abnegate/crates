use serde::{Deserialize, Serialize};

/// An image reference.
///
/// Data URLs are accepted by OpenAI-compatible providers, which is how an
/// uploaded image reaches a vision model without object storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageUrl {
    pub url: String,
}

impl ImageUrl {
    /// A reference to the image at `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}
