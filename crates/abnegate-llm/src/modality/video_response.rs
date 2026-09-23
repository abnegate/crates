use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResponse {
    pub data: Vec<u8>,
    pub format: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = VideoResponse {
            data: vec![0, 1, 2, 3],
            format: "mp4".into(),
            duration_seconds: 5.0,
            width: 1920,
            height: 1080,
            fps: 30,
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: VideoResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.format, "mp4");
        assert_eq!(roundtrip.width, 1920);
        assert_eq!(roundtrip.height, 1080);
        assert_eq!(roundtrip.fps, 30);
    }
}
