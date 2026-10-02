//! How a [`Hit`](crate::Hit) was ranked.

/// How a [`Hit`](crate::Hit) was ranked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// Cosine similarity against a caller-supplied query vector.
    Cosine,
    /// Token overlap against the query string.
    #[default]
    Keyword,
    /// Host vector index (HNSW, vectorlite, or similar).
    Vector,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_is_the_default() {
        assert_eq!(Method::default(), Method::Keyword);
    }
}
