use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageResponse {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub revised_prompt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_revised_prompt() {
        let response = ImageResponse {
            data: vec![1, 2, 3, 4, 5],
            width: 256,
            height: 256,
            format: "png".into(),
            revised_prompt: Some("revised prompt".into()),
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: ImageResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.width, 256);
        assert_eq!(roundtrip.height, 256);
        assert_eq!(roundtrip.format, "png");
        assert_eq!(roundtrip.revised_prompt.as_deref(), Some("revised prompt"));
    }

    #[test]
    fn round_trips_without_a_revised_prompt() {
        let response = ImageResponse {
            data: Vec::new(),
            width: 512,
            height: 512,
            format: "jpg".into(),
            revised_prompt: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: ImageResponse = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.revised_prompt.is_none());
    }
}
