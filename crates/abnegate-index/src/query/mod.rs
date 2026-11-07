//! Turning a chat question into a keyword query that can hit code.

mod embed;
mod identifiers;
mod natural_language;
mod path;
mod rewritten;
mod sanitize;

pub use embed::embed_query_text;
pub use natural_language::bridge_terms;
pub use natural_language::content_tokens;
pub use natural_language::question_phrases;
pub use path::path_uri_patterns;
pub use path::path_uri_tokens;
pub use rewritten::RewrittenQuery;
pub use rewritten::rewrite_query;
pub use sanitize::sanitize_search_query;
