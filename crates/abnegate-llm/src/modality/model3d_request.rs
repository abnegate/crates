use serde::{Deserialize, Serialize};

use crate::modality::Model3DFormat;

/// A 3D model for a [`Model3DProvider`](crate::Model3DProvider) to generate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Model3DRequest {
    pub prompt: String,
    pub format: Model3DFormat,
    pub reference_images: Vec<String>,
    /// How many polygons the mesh should have, if the provider takes a
    /// target. Serialised as `poly_count`.
    #[serde(rename = "poly_count")]
    pub polygon_count: Option<u32>,
}

impl Model3DRequest {
    /// A model of `prompt` in `format`, with no references and no polygon
    /// target.
    pub fn new(prompt: impl Into<String>, format: Model3DFormat) -> Self {
        Self {
            prompt: prompt.into(),
            format,
            reference_images: Vec::new(),
            polygon_count: None,
        }
    }

    /// Set the images the model should resemble.
    pub fn with_reference_images(mut self, reference_images: Vec<String>) -> Self {
        self.reference_images = reference_images;
        self
    }

    /// Set [`Self::polygon_count`].
    pub fn with_polygon_count(mut self, polygon_count: u32) -> Self {
        self.polygon_count = Some(polygon_count);
        self
    }
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
        let request = Model3DRequest::new("A treasure chest", Model3DFormat::Glb)
            .with_reference_images(vec!["ref.png".into()])
            .with_polygon_count(5000);

        let json = serde_json::to_string(&request).unwrap();
        let roundtrip: Model3DRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.prompt, "A treasure chest");
        assert!(matches!(roundtrip.format, Model3DFormat::Glb));
        assert_eq!(roundtrip.reference_images.len(), 1);
        assert_eq!(roundtrip.polygon_count, Some(5000));
    }
}
