#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Source chunking, query rewriting, learned ranking and hybrid fusion for
//! host-owned semantic code search.
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
//!
//! # Chunking
//!
//! [`chunks`] cuts a file into overlapping windows on line boundaries.
//! [`chunk_text`] cuts prose into [`TextChunk`]s of about a token budget,
//! breaking at a paragraph, sentence, line or word near the end of each
//! window.
//!
//! With the `code-chunking` feature, [`chunk_code`] parses source with
//! tree-sitter and splits it on symbol boundaries. Content in no known language,
//! too large to parse, or generated, and any chunk far over its budget, is cut
//! into text windows instead. A symbol usually gets a body chunk and a skeleton
//! of its signature, small symbols are merged and large ones split; the file
//! gets a header chunk with its imports and an API surface chunk with its
//! public signatures. Every [`CodeChunk`] carries the identifiers it defines
//! and references, so keyword and vector search see the same names.
//! [`smart_chunk`] uses the code chunker when the language is known and text
//! windows otherwise.
//!
//! ```
//! # #[cfg(feature = "code-chunking")] {
//! use abnegate_index::{CodeChunkType, CodeLanguage, chunk_code};
//!
//! let source = "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}\")\n}\n";
//! let chunks = chunk_code(source, CodeLanguage::Rust, 512);
//!
//! let body = chunks
//!     .iter()
//!     .find(|chunk| chunk.chunk_type == CodeChunkType::Body)
//!     .unwrap();
//! assert_eq!(body.metadata.symbol.as_deref(), Some("greet"));
//! # }
//! ```
//!
//! # Query rewriting
//!
//! [`rewrite_query`] turns a question into a [`RewrittenQuery`]: the code
//! identifiers it names, kept intact, and a keyword query that ORs them with
//! the question's distinctive words. A full-text index then matches
//! `should_skip_blob` instead of stemming it into `shouldskipblob`.
//!
//! ```
//! use abnegate_index::rewrite_query;
//!
//! let rewritten = rewrite_query("Where is should_skip_blob called?");
//! assert_eq!(rewritten.identifiers, ["should_skip_blob"]);
//! assert!(rewritten.keyword.contains("should_skip_blob"));
//! ```
//!
//! # Ranking
//!
//! [`score_hit`] scores one retrieved chunk. A [`LinearRanker`] scores the
//! features [`extract_features`] reads from the hit, that score is blended
//! with [`lexical_cross_score`], and a bonus rewards definitions that cover
//! the question's identifiers. [`fit_pairwise`] trains a ranker from
//! `(better, worse)` feature pairs. The weights belong to the host: this crate
//! ships no trained ranker.
//!
//! # Fusion
//!
//! The host runs the keyword and the semantic search.
//! [`reciprocal_rank_fusion`] merges the two ranked lists of [`Candidate`]s
//! by position, weighted by a [`HybridSearchConfig`], and
//! [`finalize_ranking`] reranks the fused [`HybridSearchResult`]s with a
//! ranker, drops fixtures, and keeps at most two chunks per file.
//!
//! ```
//! use abnegate_index::{
//!     Candidate, FEATURE_COUNT, HybridSearchConfig, LinearRanker, finalize_ranking,
//!     reciprocal_rank_fusion,
//! };
//!
//! let query = "Where is verify_signature defined?";
//! let keyword = vec![
//!     Candidate::new(1, "fn verify_signature() {}", "src/webhook.rs", "webhook.rs", 4.2),
//!     Candidate::new(2, "verify_signature();", "src/routes.rs", "routes.rs", 1.3),
//! ];
//! let semantic = vec![
//!     Candidate::new(1, "fn verify_signature() {}", "src/webhook.rs", "webhook.rs", 0.82),
//!     Candidate::new(3, "struct Signature;", "src/signature.rs", "signature.rs", 0.61),
//! ];
//!
//! let fused = reciprocal_rank_fusion(keyword, semantic, &HybridSearchConfig::for_query(query));
//! assert_eq!(fused[0].id, 1);
//! assert_eq!(fused[0].keyword_rank, Some(1));
//! assert_eq!(fused[0].semantic_rank, Some(1));
//!
//! let ranker = LinearRanker::new([0.0; FEATURE_COUNT], 0.0);
//! let ranked = finalize_ranking(&ranker, fused, query, 10);
//! assert_eq!(ranked[0].item_uri, "src/webhook.rs");
//! ```
//!
//! # Features
//!
//! - `code-chunking`: tree-sitter code chunking through [`chunk_code`],
//!   [`chunk_code_with_config`] and [`smart_chunk`], for Rust, Python,
//!   JavaScript, TypeScript, Go, Java, C, C++, Kotlin, Swift, SQL, JSON, YAML,
//!   Ruby and PHP. It adds one grammar crate per language, and
//!   `tree-sitter-sequel` requires `cc ~1.2`, which any crate in the same build
//!   that needs a later `cc` will conflict with.
//! - `testing`: [`Scripted`], an [`Embedder`] that returns inserted vectors so
//!   a caller can rank without loading a model.
//!
#![cfg_attr(
    feature = "code-chunking",
    doc = "[`chunk_code`]: crate::chunk_code",
    doc = "[`chunk_code_with_config`]: crate::chunk_code_with_config",
    doc = "[`smart_chunk`]: crate::smart_chunk",
    doc = "[`CodeChunk`]: crate::CodeChunk"
)]
#![cfg_attr(
    not(feature = "code-chunking"),
    doc = "[`chunk_code`]: #features",
    doc = "[`chunk_code_with_config`]: #features",
    doc = "[`smart_chunk`]: #features",
    doc = "[`CodeChunk`]: #features"
)]
#![cfg_attr(feature = "testing", doc = "[`Scripted`]: crate::Scripted")]
#![cfg_attr(not(feature = "testing"), doc = "[`Scripted`]: #features")]

mod cancel;
mod chunk;
#[cfg(feature = "code-chunking")]
mod code;
mod embedder;
mod error;
mod generated;
mod hash;
mod hit;
mod hybrid;
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
pub use crate::hybrid::Candidate;
pub use crate::hybrid::HybridSearchConfig;
pub use crate::hybrid::HybridSearchResult;
pub use crate::hybrid::apply_local_rerank;
pub use crate::hybrid::cap_per_file;
pub use crate::hybrid::finalize_ranking;
pub use crate::hybrid::identifier_file_patterns;
pub use crate::hybrid::keep_answer_chunks;
pub use crate::hybrid::prefer_definition_chunks;
pub use crate::hybrid::reciprocal_rank_fusion;
pub use crate::hybrid::same_file_patterns;
pub use crate::hybrid::sort_by_score;
pub use crate::hybrid::uri_stem_matches_identifier;
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
