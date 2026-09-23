use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEditRequest {
    pub image: Vec<u8>,
    pub mask: Option<Vec<u8>>,
    pub prompt: String,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_a_mask() {
        let request = ImageEditRequest {
            image: vec![1, 2, 3],
            mask: Some(vec![4, 5, 6]),
            prompt: "Remove the background".into(),
            width: 512,
            height: 512,
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageEditRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "Remove the background");
        assert_eq!(roundtrip.mask.as_deref(), Some([4, 5, 6].as_slice()));
    }

    #[test]
    fn round_trips_without_a_mask() {
        let request = ImageEditRequest {
            image: vec![1, 2, 3],
            mask: None,
            prompt: "Add a hat".into(),
            width: 512,
            height: 512,
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: ImageEditRequest = serde_json::from_str(&json).unwrap();

        assert!(roundtrip.mask.is_none());
    }
}
