//! Fusing keyword and semantic legs, then ranking the fused chunks per file.

mod answer;
mod candidate;
mod compare;
mod config;
mod coverage;
mod file;
mod finalize;
mod fusion;
mod fusion_score;
mod pattern;
mod question;
mod result;
#[cfg(test)]
mod sample;
mod uri;

pub use answer::keep_answer_chunks;
pub use candidate::Candidate;
pub use config::HybridSearchConfig;
pub use file::cap_per_file;
pub use finalize::apply_local_rerank;
pub use finalize::finalize_ranking;
pub use finalize::prefer_definition_chunks;
pub use finalize::sort_by_score;
pub use fusion::reciprocal_rank_fusion;
pub use pattern::identifier_file_patterns;
pub use pattern::same_file_patterns;
pub use result::HybridSearchResult;
pub use uri::uri_stem_matches_identifier;
