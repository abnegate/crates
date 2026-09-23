#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Modality-axis model providers, cost estimation and hardware profiling.
//!
//! [`modality`] holds one trait per modality — text, image, audio, voice,
//! video, 3D model, embedding and transcription — together with their request
//! and response types, the per-modality configuration, and the vendor-native
//! clients behind their features. [`cost`] picks a model for a task under a
//! [`CostStrategy`], and [`hardware`] says what a machine can run locally.

pub mod cost;
pub mod hardware;
pub mod modality;

pub use crate::cost::{
    CostEstimate, CostEstimator, CostLineItem, CostStrategy, ModelPricing, PricingUnit,
    TaskCategory, TaskSpec, default_pricing,
};
pub use crate::hardware::{GpuType, MachineProfile, ModelRecommendation, RecommendedModels};
pub use crate::modality::{
    AiClient, AiError, AudioProvider, AudioProviderConfig, AudioResponse, EmbeddingProvider,
    EmbeddingProviderConfig, Exchange, ImageEditRequest, ImageProvider, ImageProviderConfig,
    ImageRequest, ImageResponse, ModalityError, Model3DFormat, Model3DProvider,
    Model3DProviderConfig, Model3DRequest, Model3DResponse, MusicRequest, ProviderConfig,
    ResponseFormat, SfxRequest, TextProvider, TextProviderConfig, TextRequest, TextResponse,
    TranscriptionProvider, TranscriptionProviderConfig, TranscriptionResponse,
    TranscriptionSegment, VideoProvider, VideoProviderConfig, VideoRequest, VideoResponse,
    VoiceInfo, VoiceProvider, VoiceProviderConfig, VoiceRequest,
};
