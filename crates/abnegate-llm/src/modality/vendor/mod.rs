//! Vendor-native clients, one per feature.

#[cfg(feature = "anthropic")]
#[cfg_attr(docsrs, doc(cfg(feature = "anthropic")))]
pub mod anthropic;
#[cfg(feature = "fastembed")]
#[cfg_attr(docsrs, doc(cfg(feature = "fastembed")))]
pub mod fastembed;
#[cfg(feature = "google")]
#[cfg_attr(docsrs, doc(cfg(feature = "google")))]
pub mod google;
#[cfg(test)]
pub(crate) mod mock;
#[cfg(feature = "openai")]
#[cfg_attr(docsrs, doc(cfg(feature = "openai")))]
pub mod openai;
#[cfg(any(feature = "anthropic", feature = "google", feature = "openai"))]
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
#[cfg(feature = "openai")]
pub use crate::modality::vendor::openai::OpenAiProvider;
