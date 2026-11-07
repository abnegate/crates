#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Overlapping-window source chunking and host-owned semantic code search.
//!
//! [`index_tree`] walks a directory, splits each file with [`chunks`], and
//! writes windows through a [`Store`] the host implements. Embeddings are
//! optional: pass an [`Embedder`] to attach vectors, or `None` so later
//! keyword search still works. [`Memory`] is an in-memory store; persistence
//! and models stay in the host. This crate never opens a database or loads a
//! model.
//!
//! ```
//! use abnegate_index::{Hit, Memory, Method, Store};
//!
//! let store = Memory::new();
//! store
//!     .replace_file(
//!         "/src",
//!         "api.rb",
//!         "rb",
//!         "abc",
//!         &[("get ':id/entities'".into(), Some(vec![1.0, 0.0]))],
//!     )
//!     .unwrap();
//!
//! let hits: Vec<Hit> = store.search_vector(&[1.0, 0.0], Some("/src"), 4).unwrap();
//! assert_eq!(hits[0].relative, "api.rb");
//! assert_eq!(hits[0].method, Method::Cosine);
//! ```

mod cancel;
mod chunk;
#[cfg(feature = "code-chunking")]
mod code;
mod embedder;
mod error;
mod generated;
mod hash;
mod hit;
mod index;
mod language;
mod memory;
mod method;
mod proceed;
mod query;
mod rank;
mod relative;
mod similarity;
mod stats;
mod store;
mod text_chunk;
mod tokens;
mod walk;

#[cfg(any(test, feature = "testing"))]
mod scripted;

pub use crate::cancel::Cancel;
pub use crate::chunk::OVERLAP;
pub use crate::chunk::SMALLEST;
pub use crate::chunk::WINDOW;
pub use crate::chunk::chunks;
pub use crate::embedder::Embedder;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::generated::generated;
pub use crate::hash::file_hash;
pub use crate::hit::Hit;
pub use crate::index::EMBED_BATCH;
pub use crate::index::index_texts;
pub use crate::index::index_tree;
pub use crate::index::index_walk;
pub use crate::language::language_of;
pub use crate::memory::Memory;
pub use crate::method::Method;
pub use crate::proceed::Proceed;
pub use crate::query::RewrittenQuery;
pub use crate::query::bridge_terms;
pub use crate::query::content_tokens;
pub use crate::query::embed_query_text;
pub use crate::query::path_uri_patterns;
pub use crate::query::path_uri_tokens;
pub use crate::query::question_phrases;
pub use crate::query::rewrite_query;
pub use crate::query::sanitize_search_query;
pub use crate::rank::FEATURE_COUNT;
pub use crate::rank::IdentifierRole;
pub use crate::rank::LinearRanker;
pub use crate::rank::RankFeatures;
pub use crate::rank::answers_as_definition;
pub use crate::rank::blend_rank;
pub use crate::rank::chunk_rank_tier;
pub use crate::rank::definition_misses_question;
pub use crate::rank::extract_features;
pub use crate::rank::first_stage_answered;
pub use crate::rank::fit_pairwise;
pub use crate::rank::identifier_hit_count;
pub use crate::rank::identifier_part_coverage;
pub use crate::rank::identifier_role;
pub use crate::rank::is_fixture_uri;
pub use crate::rank::is_module_prelude;
pub use crate::rank::is_outline_chunk;
pub use crate::rank::is_test_chunk;
pub use crate::rank::lexical_cross_score;
pub use crate::rank::role_strength;
pub use crate::rank::score_hit;
pub use crate::relative::relative_to;
pub use crate::relative::root_key;
pub use crate::similarity::SIMILARITY_FLOOR;
pub use crate::similarity::cosine_similarity;
pub use crate::stats::Stats;
pub use crate::store::Store;
pub use crate::text_chunk::TextChunk;
pub use crate::tokens::chunk_text;
pub use crate::tokens::estimate_tokens;
pub use crate::walk::EXTENSIONS;
pub use crate::walk::LARGEST_FILE;
pub use crate::walk::SKIP_DIRECTORIES;
pub use crate::walk::Walk;
pub use crate::walk::walk_source;

#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::ChunkConfig;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::ChunkMetadata;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::CodeChunk;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::CodeChunkType;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::CodeLanguage;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::IdentifierBag;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::SymbolKind;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::Visibility;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::chunk_code;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::chunk_code_with_config;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::code_chunk_to_text_chunk;
#[cfg(feature = "code-chunking")]
#[cfg_attr(docsrs, doc(cfg(feature = "code-chunking")))]
pub use crate::code::smart_chunk;
#[cfg(feature = "testing")]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub use crate::scripted::Scripted;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
