use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRequest {
    pub prompt: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub reference_image: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_reference_image() {
        let request = VideoRequest {
            prompt: "A sunset timelapse".into(),
            duration_seconds: 10.0,
            width: 1920,
            height: 1080,
            fps: 24,
            reference_image: Some("ref.png".into()),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VideoRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A sunset timelapse");
        assert_eq!(roundtrip.fps, 24);
        assert_eq!(roundtrip.reference_image.as_deref(), Some("ref.png"));
    }

    #[test]
    fn round_trips_without_a_reference_image() {
        let request = VideoRequest {
            prompt: "test".into(),
            duration_seconds: 5.0,
            width: 512,
            height: 512,
            fps: 30,
            reference_image: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: VideoRequest = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.reference_image.is_none());
    }
}
