use serde::{Deserialize, Serialize};

use crate::modality::Model3DFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model3DRequest {
    pub prompt: String,
    pub format: Model3DFormat,
    pub reference_images: Vec<String>,
    /// How many polygons the mesh should have, if the provider takes a
    /// target. Serialised as `poly_count`.
    #[serde(rename = "poly_count")]
    pub polygon_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_polygon_count_keeps_its_serialised_name() {
        let saved =
            r#"{"prompt":"A chest","format":"Glb","reference_images":[],"poly_count":5000}"#;

        let request: Model3DRequest = serde_json::from_str(saved).unwrap();

        assert_eq!(request.polygon_count, Some(5000));
        assert_eq!(serde_json::to_string(&request).unwrap(), saved);
    }

    #[test]
    fn round_trips_through_json() {
        let request = Model3DRequest {
            prompt: "A treasure chest".into(),
            format: Model3DFormat::Glb,
            reference_images: vec!["ref.png".into()],
            polygon_count: Some(5000),
        };

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: Model3DRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A treasure chest");
        assert_eq!(roundtrip.reference_images.len(), 1);
        assert_eq!(roundtrip.polygon_count, Some(5000));
    }
}
