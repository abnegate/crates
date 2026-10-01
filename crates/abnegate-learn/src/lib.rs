#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! In-memory trial memory: record attempts, retrieve similar outcomes, and
//! turn failures into avoid/context suggestions for the next round.
//!
//! [`Memory`] holds [`Trial`] rows. A host records each attempt with
//! [`TrialInput`], optionally attaches an embedding (or an [`Embedder`]), and
//! asks [`Memory::digest`] for the scoped picture of what already failed.
//! Similarity uses cosine on those embeddings; a trial with no embedding still
//! contributes to clusters and the failed-strategy list. Persistence is an
//! [`Archive`] the host implements; this crate never opens a database.
//!
//! ```
//! use abnegate_learn::{Memory, TrialInput, Verdict};
//!
//! let mut memory = Memory::new();
//! memory.record(
//!     TrialInput::new("lab", "fuzz")
//!         .with_verdict(Verdict::Skip)
//!         .with_error("reprl-unavailable")
//!         .with_lesson("fuzz cannot run without reprl")
//!         .with_embedding(vec![1.0, 0.0]),
//! );
//!
//! let digest = memory.digest("lab", Some(&[1.0, 0.0]));
//! assert!(digest.failed_strategies.iter().any(|name| name == "fuzz"));
//! assert!(digest.as_prompt().contains("fuzz"));
//! ```

mod advisor;
mod archive;
mod category;
mod class;
mod cluster;
mod config;
mod digest;
mod embedder;
mod error;
mod extractor;
mod fingerprint;
mod kind;
mod lesson;
mod memory;
mod similar;
mod similarity;
mod suggestion;
mod tag;
mod trial;
mod trial_input;
mod verdict;

pub use crate::advisor::Advisor;
pub use crate::archive::Archive;
pub use crate::category::Category;
pub use crate::class::ErrorClass;
pub use crate::cluster::Cluster;
pub use crate::config::Config;
pub use crate::digest::Digest;
pub use crate::embedder::Embedder;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::extractor::Extractor;
pub use crate::fingerprint::Fingerprint;
pub use crate::kind::SuggestionKind;
pub use crate::lesson::Lesson;
pub use crate::memory::Memory;
pub use crate::similar::SimilarTrial;
pub use crate::similarity::cosine_similarity;
pub use crate::similarity::euclidean_distance;
pub use crate::similarity::normalize;
pub use crate::suggestion::Suggestion;
pub use crate::tag::is_common;
pub use crate::tag::tags_from;
pub use crate::trial::Trial;
pub use crate::trial_input::TrialInput;
pub use crate::verdict::Verdict;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
