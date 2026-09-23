use serde::{Deserialize, Serialize};

use crate::modality::Model3DFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model3DResponse {
    pub data: Vec<u8>,
    pub format: Model3DFormat,
    pub vertex_count: u32,
    pub face_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let response = Model3DResponse {
            data: vec![10, 20, 30],
            format: Model3DFormat::Obj,
            vertex_count: 1500,
            face_count: 3000,
        };

        let json = serde_json::to_string(&response).unwrap();
        let roundtrip: Model3DResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtrip.vertex_count, 1500);
        assert_eq!(roundtrip.face_count, 3000);
        assert!(matches!(roundtrip.format, Model3DFormat::Obj));
    }
}
