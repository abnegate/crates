use serde::{Deserialize, Serialize};

/// A clip for a [`VideoProvider`](crate::VideoProvider) to generate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VideoRequest {
    pub prompt: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub reference_image: Option<String>,
}

impl VideoRequest {
    /// `duration_seconds` of `prompt` at `width` by `height` and `fps` frames a
    /// second, with no reference image.
    pub fn new(
        prompt: impl Into<String>,
        duration_seconds: f64,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            duration_seconds,
            width,
            height,
            fps,
            reference_image: None,
        }
    }

    /// Start the clip from, or style it after, `reference_image`.
    pub fn with_reference_image(mut self, reference_image: impl Into<String>) -> Self {
        self.reference_image = Some(reference_image.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_reference_image() {
        let request = VideoRequest::new("A sunset timelapse", 10.0, 1920, 1080, 24)
            .with_reference_image("ref.png");

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VideoRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A sunset timelapse");
        assert!((roundtrip.duration_seconds - 10.0).abs() < f64::EPSILON);
        assert_eq!((roundtrip.width, roundtrip.height), (1920, 1080));
        assert_eq!(roundtrip.fps, 24);
        assert_eq!(roundtrip.reference_image.as_deref(), Some("ref.png"));
    }

    #[test]
    fn round_trips_without_a_reference_image() {
        let request = VideoRequest::new("test", 5.0, 512, 512, 30);

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VideoRequest = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.reference_image.is_none());
    }
}
