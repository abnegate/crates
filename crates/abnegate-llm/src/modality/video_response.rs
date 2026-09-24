use serde::{Deserialize, Serialize};

/// A clip a [`VideoProvider`](crate::VideoProvider) made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VideoResponse {
    pub data: Vec<u8>,
    pub format: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl VideoResponse {
    /// `duration_seconds` of `width` by `height` video at `fps` frames a
    /// second, encoded as `format`, such as `mp4`.
    pub fn new(
        data: Vec<u8>,
        format: impl Into<String>,
        duration_seconds: f64,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Self {
        Self {
            data,
            format: format.into(),
            duration_seconds,
            width,
            height,
            fps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = VideoResponse::new(vec![0, 1, 2, 3], "mp4", 5.0, 1920, 1080, 30);

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: VideoResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.data, vec![0, 1, 2, 3]);
        assert_eq!(roundtrip.format, "mp4");
        assert!((roundtrip.duration_seconds - 5.0).abs() < f64::EPSILON);
        assert_eq!(roundtrip.width, 1920);
        assert_eq!(roundtrip.height, 1080);
        assert_eq!(roundtrip.fps, 30);
    }
}
