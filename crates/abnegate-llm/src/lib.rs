#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! An OpenAI-compatible chat completions client, a provider abstraction that
//! puts several of them behind one handle, and one trait per generative
//! modality for the vendors that do not speak that API.
//!
//! [`LlmClient`] speaks the chat completions API, streaming or not, over a
//! connection pool held per runtime so keep-alives survive between turns.
//! [`CompletionProvider`] is the contract a source of completions satisfies,
//! and [`Router`] satisfies it over a set of providers, so a consumer never
//! learns whether it is talking to one model, an A/B split, or a fallback
//! chain three deep.
//!
//! [`modality`] holds one trait per modality — text, image, audio, voice,
//! video, 3D model, embedding and transcription — together with their request
//! and response types, the per-modality configuration, and the vendor-native
//! clients behind their features. [`cost`] picks a model for a task under a
//! [`CostStrategy`], [`hardware`] says what a machine can run locally, and
//! [`catalog`] browses the model catalogues those choices are made from.
//!
//! ```no_run
//! use abnegate_llm::{LlmClient, LlmConfig, Message};
//!
//! # async fn example() -> Result<(), abnegate_llm::LlmError> {
//! let client = LlmClient::new(LlmConfig::new("http://127.0.0.1:4000/v1", "qwen3", ""));
//!
//! let response = client.chat(&[Message::user("Say hello.")], None).await?;
//! println!("{:?}", response.choices[0].message.content);
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `anthropic`: the Anthropic messages client behind [`modality::TextProvider`].
//! - `google`: the Gemini client behind [`modality::TextProvider`].
//! - `openai`: the OpenAI client behind the text, image, embedding and
//!   transcription traits.
//! - `catalog`: browse the Ollama library, HuggingFace, GPT4All and OpenRouter
//!   catalogues through one [`catalog::ModelProvider`] trait.
//! - `testing`: [`provider::testing::StubProvider`], a completion provider
//!   whose answers a test decides, for crates that route or wrap providers.
//! - `download`: resumable GGUF downloads that only splice a resume onto the
//!   same upstream file and verify a SHA-256 when one is given.

#[cfg(feature = "catalog")]
#[cfg_attr(docsrs, doc(cfg(feature = "catalog")))]
pub mod catalog;
mod client;
pub mod cost;
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub mod download;
mod error;
pub mod hardware;
pub mod history;
pub mod modality;
pub mod provider;
mod reasoning;
mod wire;

pub use crate::client::{LlmClient, LlmConfig, RequestOptions};
pub use crate::cost::{
    CostEstimate, CostEstimator, CostLineItem, CostStrategy, ModelPricing, PricingUnit,
    TaskCategory, TaskSpec, default_pricing,
};
pub use crate::error::LlmError;
pub use crate::hardware::{GpuType, MachineProfile, ModelRecommendation, RecommendedModels};
pub use crate::modality::{
    AiClient, AiError, AudioProvider, AudioProviderConfig, AudioResponse, CompletionBridge,
    EmbeddingProvider, EmbeddingProviderConfig, Exchange, ImageEditRequest, ImageProvider,
    ImageProviderConfig, ImageRequest, ImageResponse, Model3DFormat, Model3DProvider,
    Model3DProviderConfig, Model3DRequest, Model3DResponse, MusicRequest, ProviderConfig,
    ResponseFormat, SfxRequest, StructuredResponse, TextProvider, TextProviderConfig, TextRequest,
    TextResponse, TranscriptionProvider, TranscriptionProviderConfig, TranscriptionResponse,
    TranscriptionSegment, VideoProvider, VideoProviderConfig, VideoRequest, VideoResponse,
    VoiceInfo, VoiceProvider, VoiceProviderConfig, VoiceRequest,
};
pub use crate::provider::{
    Capabilities, Completion, CompletionProvider, CompletionRequest, Credential, ExitStatus,
    HttpProvider, ProviderError, ProviderKind, Router, SelectionStrategy, Weighted, choose, sample,
};
pub use crate::reasoning::{Effort, ReasoningEffort, classify};
pub use crate::wire::{
    ChatRequest, ChatResponse, ChatStreamChunk, Choice, ContentPart, FunctionCall,
    FunctionDefinition, GeneratedImage, ImageUrl, Message, Role, SpecificFunction, StreamChoice,
    StreamDelta, StreamFunctionCall, StreamToolCall, ToolCall, ToolChoice, ToolDefinition, Usage,
    wire_arguments,
};
