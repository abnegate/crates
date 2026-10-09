use serde::Deserialize;
use serde::Serialize;

/// The recipe binding written beside a weight, which says which recipe runs it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct WeightSidecar {
    /// The recipe that runs the weight.
    pub recipe_id: String,
    /// The Hugging Face base model a LoRA was trained on.
    #[serde(default, rename = "hf_base")]
    pub huggingface_base: Option<String>,
    /// The trigger word the weight was trained on, which selects it when a
    /// prompt names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// The textual-inversion embedding trained beside the weight, a file in
    /// the models directory's `embeddings/`, whose stem leads every prompt
    /// run on the weight as `embedding:<stem>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<String>,
    /// The face reference beside the weight that an identity's generation
    /// steers its face with, when its name is not the weight's stem with
    /// [`FACE_SUFFIX`](crate::inventory::FACE_SUFFIX).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<String>,
}

impl WeightSidecar {
    /// Binds a weight to the recipe `recipe_id`, naming no base model.
    pub fn new(recipe_id: impl Into<String>) -> Self {
        Self {
            recipe_id: recipe_id.into(),
            huggingface_base: None,
            trigger: None,
            embedding: None,
            face: None,
        }
    }

    /// Names the Hugging Face base model the weight was trained on, which a
    /// LoRA's binding needs before the inventory lists it.
    pub fn with_huggingface_base(mut self, base: impl Into<String>) -> Self {
        self.huggingface_base = Some(base.into());
        self
    }

    /// Names the trigger word the weight was trained on, so a prompt that
    /// names it selects the weight.
    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }

    /// Names the textual-inversion embedding trained with the weight.
    pub fn with_embedding(mut self, embedding: impl Into<String>) -> Self {
        self.embedding = Some(embedding.into());
        self
    }

    /// Names the face reference beside the weight.
    pub fn with_face(mut self, face: impl Into<String>) -> Self {
        self.face = Some(face.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_sidecar_keeps_its_embedding_and_face() {
        let sidecar: WeightSidecar = serde_json::from_str(
            r#"{"recipe_id":"sdxl-adapter","hf_base":"stabilityai/stable-diffusion-xl-base-1.0","trigger":"ohwx","embedding":"ohwx.safetensors","face":"jerry.face.png"}"#,
        )
        .unwrap();
        assert_eq!(sidecar.embedding.as_deref(), Some("ohwx.safetensors"));
        assert_eq!(sidecar.face.as_deref(), Some("jerry.face.png"));
        assert_eq!(
            sidecar,
            WeightSidecar::new("sdxl-adapter")
                .with_huggingface_base("stabilityai/stable-diffusion-xl-base-1.0")
                .with_trigger("ohwx")
                .with_embedding("ohwx.safetensors")
                .with_face("jerry.face.png")
        );
    }

    #[test]
    fn a_sidecar_without_them_is_written_as_before() {
        let sidecar = WeightSidecar::new("flux-schnell-adapter");
        let written = serde_json::to_value(&sidecar).unwrap();
        assert_eq!(
            written,
            serde_json::json!({"recipe_id": "flux-schnell-adapter", "hf_base": null})
        );
        let read: WeightSidecar = serde_json::from_value(written).unwrap();
        assert_eq!(read.embedding, None);
        assert_eq!(read.face, None);
    }
}
