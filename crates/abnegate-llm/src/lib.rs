#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! An OpenAI-compatible chat completions client, and a provider abstraction
//! that puts several of them behind one handle.
//!
//! [`LlmClient`] speaks the chat completions API, streaming or not, over a
//! connection pool held per runtime so keep-alives survive between turns.
//! [`CompletionProvider`] is the contract a source of completions satisfies,
//! and [`Router`] satisfies it over a set of providers, so a consumer never
//! learns whether it is talking to one model, an A/B split, or a fallback
//! chain three deep.
//!
//! ```no_run
//! use abnegate_llm::{LlmClient, LlmConfig, Message};
//!
//! # async fn example() -> Result<(), abnegate_llm::LlmError> {
//! let client = LlmClient::new(LlmConfig {
//!     base_url: "http://127.0.0.1:4000/v1".to_string(),
//!     default_model: "qwen3".to_string(),
//!     ..LlmConfig::default()
//! });
//!
//! let response = client.chat(&[Message::user("Say hello.")], None).await?;
//! println!("{:?}", response.choices[0].message.content);
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
pub mod history;
pub mod provider;
mod reasoning;
mod wire;

pub use crate::client::{LlmClient, LlmConfig, RequestOptions};
pub use crate::error::LlmError;
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
