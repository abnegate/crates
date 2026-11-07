//! Vendor-native clients, one per feature.

#[cfg(feature = "anthropic")]
#[cfg_attr(docsrs, doc(cfg(feature = "anthropic")))]
pub mod anthropic;
#[cfg(any(feature = "ollama", feature = "openai-compatible", feature = "rerank"))]
mod endpoint;
#[cfg(feature = "fastembed")]
#[cfg_attr(docsrs, doc(cfg(feature = "fastembed")))]
pub mod fastembed;
#[cfg(feature = "google")]
#[cfg_attr(docsrs, doc(cfg(feature = "google")))]
pub mod google;
#[cfg(test)]
pub(crate) mod mock;
#[cfg(feature = "ollama")]
#[cfg_attr(docsrs, doc(cfg(feature = "ollama")))]
pub mod ollama;
#[cfg(feature = "openai")]
#[cfg_attr(docsrs, doc(cfg(feature = "openai")))]
pub mod openai;
#[cfg(feature = "openai-compatible")]
#[cfg_attr(docsrs, doc(cfg(feature = "openai-compatible")))]
pub mod openai_compatible;
#[cfg(feature = "rerank")]
#[cfg_attr(docsrs, doc(cfg(feature = "rerank")))]
pub mod rerank;
#[cfg(any(
    feature = "anthropic",
    feature = "google",
    feature = "ollama",
    feature = "openai",
    feature = "openai-compatible",
    feature = "rerank"
))]
mod transport;

#[cfg(feature = "anthropic")]
pub use crate::modality::vendor::anthropic::AnthropicAuthentication;
#[cfg(feature = "anthropic")]
pub use crate::modality::vendor::anthropic::AnthropicProvider;
#[cfg(feature = "fastembed")]
pub use crate::modality::vendor::fastembed::FastembedConfig;
#[cfg(feature = "fastembed")]
pub use crate::modality::vendor::fastembed::FastembedProvider;
#[cfg(feature = "google")]
pub use crate::modality::vendor::google::GeminiProvider;
#[cfg(test)]
pub(crate) use crate::modality::vendor::mock::MockProvider;
#[cfg(feature = "ollama")]
pub use crate::modality::vendor::ollama::OllamaEmbeddingConfig;
#[cfg(feature = "ollama")]
pub use crate::modality::vendor::ollama::OllamaEmbeddingProvider;
#[cfg(feature = "openai")]
pub use crate::modality::vendor::openai::OpenAiProvider;
#[cfg(feature = "openai-compatible")]
pub use crate::modality::vendor::openai_compatible::OpenAiCompatibleEmbeddingConfig;
#[cfg(feature = "openai-compatible")]
pub use crate::modality::vendor::openai_compatible::OpenAiCompatibleEmbeddingProvider;
#[cfg(feature = "rerank")]
pub use crate::modality::vendor::rerank::HttpRerankConfig;
#[cfg(feature = "rerank")]
pub use crate::modality::vendor::rerank::HttpRerankProvider;
