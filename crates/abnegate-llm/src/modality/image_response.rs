use serde::{Deserialize, Serialize};

/// An image an [`ImageProvider`](crate::ImageProvider) made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageResponse {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub revised_prompt: Option<String>,
}

impl ImageResponse {
    /// A `width` by `height` image encoded as `format`, such as `png`.
    pub fn new(data: Vec<u8>, width: u32, height: u32, format: impl Into<String>) -> Self {
        Self {
            data,
            width,
            height,
            format: format.into(),
            revised_prompt: None,
        }
    }

    /// Set the prompt the provider actually drew, when it rewrote the one it
    /// was given.
    pub fn with_revised_prompt(mut self, revised_prompt: impl Into<String>) -> Self {
        self.revised_prompt = Some(revised_prompt.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_revised_prompt() {
        let response = ImageResponse::new(vec![1, 2, 3, 4, 5], 256, 128, "png")
            .with_revised_prompt("revised prompt");

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: ImageResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.data, vec![1, 2, 3, 4, 5]);
        assert_eq!(roundtrip.width, 256);
        assert_eq!(roundtrip.height, 128);
        assert_eq!(roundtrip.format, "png");
        assert_eq!(roundtrip.revised_prompt.as_deref(), Some("revised prompt"));
    }

    #[test]
    fn round_trips_without_a_revised_prompt() {
        let response = ImageResponse::new(Vec::new(), 512, 512, "jpg");

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: ImageResponse = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.revised_prompt.is_none());
    }
}
