//! One search result.

/// One result row to inject into the model prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchHit {
    /// Title of the page. A hit read from SearXNG carries its URL here when
    /// the engine gave no title.
    pub title: String,
    /// Address of the page.
    pub url: String,
    /// The engine's excerpt of the page, empty when it gave none.
    pub snippet: String,
    /// Namespaced citation identifier the host minted for this turn, such as
    /// `web:a3f21c`. Rendered verbatim; this crate never mints or prefixes one.
    pub identifier: Option<String>,
}

impl SearchHit {
    /// A hit with no citation identifier, so it is listed by its position.
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            identifier: None,
        }
    }

    /// This hit, cited by `identifier` instead of its position.
    pub fn with_identifier(mut self, identifier: impl Into<String>) -> Self {
        self.identifier = Some(identifier.into());
        self
    }
}
