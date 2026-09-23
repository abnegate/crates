#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Shared building blocks for talking to large language model providers.
//!
//! # Features
//!
//! - `catalog`: browse the Ollama library, HuggingFace, GPT4All and OpenRouter
//!   catalogues through one [`catalog::ModelProvider`] trait.
//! - `download`: resumable chunked GGUF downloads.

#[cfg(feature = "catalog")]
#[cfg_attr(docsrs, doc(cfg(feature = "catalog")))]
pub mod catalog;
