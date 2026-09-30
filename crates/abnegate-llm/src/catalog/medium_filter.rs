use serde::Deserialize;
use serde::Serialize;

/// High-level medium a browse is restricted to.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelMediumFilter {
    /// Every model. The default.
    #[default]
    All,
    /// Models that read text or chat, including ones whose capabilities are
    /// unknown.
    Text,
    /// Models that read or generate images.
    Image,
    /// Models that generate images.
    ImageGeneration,
    /// Models that read or generate video.
    Video,
    /// Models that read or generate audio.
    Audio,
    /// Models that call tools.
    Tools,
    /// Embedding models.
    Embeddings,
    /// Models that think before they answer.
    Reasoning,
}
