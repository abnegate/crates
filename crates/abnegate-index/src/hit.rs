//! One ranked chunk from a [`Store`](crate::Store).

use crate::method::Method;

/// One ranked chunk from a [`Store`](crate::Store).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Hit {
    /// Store-assigned identifier, such as a database row id.
    pub id: i64,
    /// Tree the chunk was indexed under.
    pub root: String,
    /// Path relative to [`root`](Self::root), using `/`.
    pub relative: String,
    /// File extension, lowercased, empty when the path has none.
    pub language: String,
    /// Window text.
    pub text: String,
    /// Rank score. Cosine is `-1.0..=1.0`; keyword is `0.0..=1.0`.
    pub score: f64,
    /// How the store produced this hit.
    pub method: Method,
}

impl Hit {
    /// A keyword hit with score `0.0`.
    pub fn new(
        id: i64,
        root: impl Into<String>,
        relative: impl Into<String>,
        language: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id,
            root: root.into(),
            relative: relative.into(),
            language: language.into(),
            text: text.into(),
            score: 0.0,
            method: Method::Keyword,
        }
    }

    /// This hit scored `score`.
    pub fn with_score(mut self, score: f64) -> Self {
        self.score = score;
        self
    }

    /// This hit ranked by `method`.
    pub fn with_method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_fill_score_and_method() {
        let hit = Hit::new(3, "/src", "api.rb", "rb", "get ':id/entities'")
            .with_score(0.91)
            .with_method(Method::Cosine);
        assert_eq!(hit.id, 3);
        assert_eq!(hit.relative, "api.rb");
        assert!((hit.score - 0.91).abs() < 1e-9);
        assert_eq!(hit.method, Method::Cosine);
    }
}
