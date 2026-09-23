use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Model3DFormat {
    Glb,
    Gltf,
    Obj,
    Fbx,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips() {
        for format in [
            Model3DFormat::Glb,
            Model3DFormat::Gltf,
            Model3DFormat::Obj,
            Model3DFormat::Fbx,
        ] {
            let json = serde_json::to_string(&format).unwrap();
            let roundtrip: Model3DFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&roundtrip).unwrap());
        }
    }
}
