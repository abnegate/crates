use serde::Deserialize;
use serde::Serialize;

/// High-level medium a browse is restricted to.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelMediumFilter {
    #[default]
    All,
    Text,
    Image,
    ImageGeneration,
    Video,
    Audio,
    Tools,
    Embeddings,
    Reasoning,
}
