//! One search result.

/// One result row to inject into the model prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Namespaced citation identifier the host minted for this turn, such as
    /// `web:a3f21c`. Rendered verbatim; this crate never mints or prefixes one.
    pub identifier: Option<String>,
}
