#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
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
//! [`CostStrategy`], [`hardware`] says what a machine can run locally, and the
//! `catalog` module, behind its feature, browses the model catalogues those
//! choices are made from.
//!
//! # Two provider contracts
//!
//! [`CompletionProvider`] takes a conversation — messages, tools, a response
//! format — and is what an OpenAI-compatible endpoint ([`HttpProvider`]), a
//! [`Router`] or a coding agent CLI in a crate layered on this one satisfies.
//! [`TextProvider`] takes one system and one user prompt and is what the
//! vendor-native clients satisfy. [`CompletionBridge`] turns any
//! `CompletionProvider` into a `TextProvider`, so [`AiClient`] runs on either.
//!
//! # Vendors
//!
//! The vendor-native clients shipped here are Anthropic (the messages API, or
//! the Claude Code CLI for an OAuth token), Gemini and OpenAI, each behind its
//! feature. Every OpenAI-compatible server — Ollama, LiteLLM, vLLM, a gateway —
//! is reached through [`HttpProvider`] instead. [`ProviderConfig`] can name
//! any provider, but it is configuration only: nothing here constructs a
//! provider from it, and a name outside this set is one the caller supplies.
//!
//! ```no_run
//! use abnegate_llm::{LlmClient, LlmConfig, Message};
//!
//! # async fn example() -> Result<(), abnegate_llm::Error> {
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
//! - `anthropic`: the Anthropic messages client behind [`TextProvider`].
//! - `google`: the Gemini client behind [`TextProvider`].
//! - `openai`: the OpenAI client behind the text, image, embedding and
//!   transcription traits.
//! - `catalog`: browse the Ollama library, HuggingFace, GPT4All and OpenRouter
//!   catalogues through one `catalog::ModelProvider` trait.
//! - `download`: resumable GGUF downloads that only splice a resume onto the
//!   same upstream file and verify a SHA-256 when one is given.
//! - `testing`: `provider::testing::StubProvider`, a completion provider whose
//!   answers a test decides, for crates that route or wrap providers.

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
mod parse_error;
pub mod provider;
pub mod reasoning;
mod wire;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use crate::client::LlmClient;
pub use crate::client::LlmConfig;
pub use crate::client::RequestOptions;
pub use crate::cost::CostEstimate;
pub use crate::cost::CostEstimator;
pub use crate::cost::CostLineItem;
pub use crate::cost::CostStrategy;
pub use crate::cost::ModelPricing;
pub use crate::cost::PricingUnit;
pub use crate::cost::TaskCategory;
pub use crate::cost::TaskSpecification;
pub use crate::cost::default_pricing;
pub use crate::error::Error;
pub use crate::hardware::GpuType;
pub use crate::hardware::MachineProfile;
pub use crate::hardware::ModelRecommendation;
pub use crate::hardware::RecommendedModels;
pub use crate::modality::AiClient;
pub use crate::modality::AiError;
pub use crate::modality::AudioProvider;
pub use crate::modality::AudioProviderConfig;
pub use crate::modality::AudioResponse;
pub use crate::modality::CompletionBridge;
pub use crate::modality::EmbeddingProvider;
pub use crate::modality::EmbeddingProviderConfig;
pub use crate::modality::Exchange;
pub use crate::modality::ImageEditRequest;
pub use crate::modality::ImageProvider;
pub use crate::modality::ImageProviderConfig;
pub use crate::modality::ImageRequest;
pub use crate::modality::ImageResponse;
pub use crate::modality::Model3DFormat;
pub use crate::modality::Model3DProvider;
pub use crate::modality::Model3DProviderConfig;
pub use crate::modality::Model3DRequest;
pub use crate::modality::Model3DResponse;
pub use crate::modality::MusicRequest;
pub use crate::modality::ProviderConfig;
pub use crate::modality::ResponseFormat;
pub use crate::modality::SoundEffectRequest;
pub use crate::modality::StructuredResponse;
pub use crate::modality::TextProvider;
pub use crate::modality::TextProviderConfig;
pub use crate::modality::TextRequest;
pub use crate::modality::TextResponse;
pub use crate::modality::TranscriptionProvider;
pub use crate::modality::TranscriptionProviderConfig;
pub use crate::modality::TranscriptionResponse;
pub use crate::modality::TranscriptionSegment;
pub use crate::modality::VideoProvider;
pub use crate::modality::VideoProviderConfig;
pub use crate::modality::VideoRequest;
pub use crate::modality::VideoResponse;
pub use crate::modality::Voice;
pub use crate::modality::VoiceProvider;
pub use crate::modality::VoiceProviderConfig;
pub use crate::modality::VoiceRequest;
pub use crate::parse_error::ParseError;
pub use crate::provider::Capabilities;
pub use crate::provider::Completion;
pub use crate::provider::CompletionProvider;
pub use crate::provider::CompletionRequest;
pub use crate::provider::Credential;
pub use crate::provider::ExitStatus;
pub use crate::provider::HttpProvider;
pub use crate::provider::ProviderError;
pub use crate::provider::ProviderKind;
pub use crate::provider::Router;
pub use crate::provider::SelectionStrategy;
pub use crate::provider::Weighted;
pub use crate::reasoning::Effort;
pub use crate::reasoning::ReasoningEffort;
pub use crate::wire::ChatRequest;
pub use crate::wire::ChatResponse;
pub use crate::wire::ChatStreamChunk;
pub use crate::wire::Choice;
pub use crate::wire::ContentPart;
pub use crate::wire::FunctionCall;
pub use crate::wire::FunctionDefinition;
pub use crate::wire::GeneratedImage;
pub use crate::wire::ImageUrl;
pub use crate::wire::Message;
pub use crate::wire::Role;
pub use crate::wire::SpecificFunction;
pub use crate::wire::StreamChoice;
pub use crate::wire::StreamDelta;
pub use crate::wire::StreamFunctionCall;
pub use crate::wire::StreamToolCall;
pub use crate::wire::ToolCall;
pub use crate::wire::ToolChoice;
pub use crate::wire::ToolDefinition;
pub use crate::wire::ToolMode;
pub use crate::wire::Usage;
pub use crate::wire::wire_arguments;
