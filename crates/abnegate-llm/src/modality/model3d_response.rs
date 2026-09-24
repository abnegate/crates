use serde::{Deserialize, Serialize};

use crate::modality::Model3DFormat;

/// A 3D model a [`Model3DProvider`](crate::Model3DProvider) made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Model3DResponse {
    pub data: Vec<u8>,
    pub format: Model3DFormat,
    pub vertex_count: u32,
    pub face_count: u32,
}

impl Model3DResponse {
    /// A mesh in `format` with `vertex_count` vertices and `face_count` faces.
    pub fn new(data: Vec<u8>, format: Model3DFormat, vertex_count: u32, face_count: u32) -> Self {
        Self {
            data,
            format,
            vertex_count,
            face_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = Model3DResponse::new(vec![10, 20, 30], Model3DFormat::Obj, 1500, 3000);

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: Model3DResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.data, vec![10, 20, 30]);
        assert_eq!(roundtrip.vertex_count, 1500);
        assert_eq!(roundtrip.face_count, 3000);
        assert!(matches!(roundtrip.format, Model3DFormat::Obj));
    }
}
