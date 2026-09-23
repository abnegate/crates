use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRequest {
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub width: u32,
    pub height: u32,
    pub style: Option<String>,
    pub reference_images: Vec<String>,
    /// How many images to generate. Read as `num_images` too.
    #[serde(alias = "num_images")]
    pub image_count: u32,
}

impl ImageRequest {
    pub fn new(prompt: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            prompt: prompt.into(),
            negative_prompt: None,
            width,
            height,
            style: None,
            reference_images: Vec::new(),
            image_count: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_in_the_defaults() {
        let request = ImageRequest::new("A cat", 512, 512);
        assert_eq!(request.prompt, "A cat");
        assert_eq!(request.width, 512);
        assert_eq!(request.height, 512);
        assert_eq!(request.image_count, 1);
        assert!(request.negative_prompt.is_none());
        assert!(request.style.is_none());
        assert!(request.reference_images.is_empty());
    }

    #[test]
    fn round_trips_with_every_option_set() {
        let request = ImageRequest {
            prompt: "A beautiful landscape".into(),
            negative_prompt: Some("blurry".into()),
            width: 1024,
            height: 768,
            style: Some("photographic".into()),
            reference_images: vec!["ref1.png".into(), "ref2.png".into()],
            image_count: 4,
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A beautiful landscape");
        assert_eq!(roundtrip.negative_prompt.as_deref(), Some("blurry"));
        assert_eq!(roundtrip.width, 1024);
        assert_eq!(roundtrip.height, 768);
        assert_eq!(roundtrip.style.as_deref(), Some("photographic"));
        assert_eq!(roundtrip.reference_images.len(), 2);
        assert_eq!(roundtrip.image_count, 4);
    }

    #[test]
    fn zero_dimensions_survive_a_round_trip() {
        let request = ImageRequest::new("A mountain", 0, 0);
        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.width, 0);
        assert_eq!(roundtrip.height, 0);
    }
}
