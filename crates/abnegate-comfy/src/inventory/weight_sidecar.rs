use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeightSidecar {
    pub recipe_id: String,
    #[serde(default, rename = "hf_base")]
    pub huggingface_base: Option<String>,
}
