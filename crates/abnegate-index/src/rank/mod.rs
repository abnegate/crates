//! Learned ranking of retrieved code chunks.

mod chunk;
mod definition;
mod features;
mod identifier;
mod lexical;
mod linear;
mod role;
mod score;

pub use chunk::chunk_rank_tier;
pub use chunk::is_fixture_uri;
pub use chunk::is_module_prelude;
pub use chunk::is_outline_chunk;
pub use chunk::is_test_chunk;
pub use definition::answers_as_definition;
pub use definition::definition_misses_question;
pub use definition::first_stage_answered;
pub use features::FEATURE_COUNT;
pub use features::RankFeatures;
pub use features::extract_features;
pub use identifier::identifier_hit_count;
pub use identifier::identifier_part_coverage;
pub use identifier::identifier_role;
pub use identifier::role_strength;
pub use lexical::blend_rank;
pub use lexical::lexical_cross_score;
pub use linear::LinearRanker;
pub use linear::fit_pairwise;
pub use role::IdentifierRole;
pub use score::score_hit;
