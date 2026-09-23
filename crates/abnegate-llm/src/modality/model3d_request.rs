use serde::{Deserialize, Serialize};

use crate::modality::Model3DFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model3DRequest {
    pub prompt: String,
    pub format: Model3DFormat,
    pub reference_images: Vec<String>,
    pub poly_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let request = Model3DRequest {
            prompt: "A treasure chest".into(),
            format: Model3DFormat::Glb,
            reference_images: vec!["ref.png".into()],
            poly_count: Some(5000),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: Model3DRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A treasure chest");
        assert_eq!(roundtrip.reference_images.len(), 1);
        assert_eq!(roundtrip.poly_count, Some(5000));
    }
}
