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
}

impl WeightSidecar {
    /// Binds a weight to the recipe `recipe_id`, naming no base model.
    pub fn new(recipe_id: impl Into<String>) -> Self {
        Self {
            recipe_id: recipe_id.into(),
            huggingface_base: None,
        }
    }

    /// Names the Hugging Face base model the weight was trained on, which a
    /// LoRA's binding needs before the inventory lists it.
    pub fn with_huggingface_base(mut self, base: impl Into<String>) -> Self {
        self.huggingface_base = Some(base.into());
        self
    }
}
