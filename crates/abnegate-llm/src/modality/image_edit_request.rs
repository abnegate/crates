use serde::{Deserialize, Serialize};

/// A change for an [`ImageProvider`](crate::ImageProvider) to make to an
/// existing image.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageEditRequest {
    pub image: Vec<u8>,
    pub mask: Option<Vec<u8>>,
    pub prompt: String,
    pub width: u32,
    pub height: u32,
}

impl ImageEditRequest {
    /// Edit the whole of `image` as `prompt` says, into a `width` by `height`
    /// result.
    pub fn new(image: Vec<u8>, prompt: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            image,
            mask: None,
            prompt: prompt.into(),
            width,
            height,
        }
    }

    /// Edit only the area `mask` marks.
    pub fn with_mask(mut self, mask: Vec<u8>) -> Self {
        self.mask = Some(mask);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_mask() {
        let request = ImageEditRequest::new(vec![1, 2, 3], "Remove the background", 512, 512)
            .with_mask(vec![4, 5, 6]);

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageEditRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.image, vec![1, 2, 3]);
        assert_eq!(roundtrip.prompt, "Remove the background");
        assert_eq!(roundtrip.mask.as_deref(), Some([4, 5, 6].as_slice()));
        assert_eq!((roundtrip.width, roundtrip.height), (512, 512));
    }

    #[test]
    fn round_trips_without_a_mask() {
        let request = ImageEditRequest::new(vec![1, 2, 3], "Add a hat", 512, 512);

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageEditRequest = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.mask.is_none());
    }
}
