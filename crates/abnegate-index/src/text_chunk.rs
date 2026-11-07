//! A span of text with its byte offsets in the source it came from.

use crate::tokens::estimate_tokens;

/// A text chunk with position information.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TextChunk {
    /// Index of this chunk, counted from 0.
    pub index: usize,
    /// The chunk text.
    pub text: String,
    /// Start byte offset in the original text.
    pub start_offset: usize,
    /// End byte offset in the original text.
    pub end_offset: usize,
}

impl TextChunk {
    /// The chunk at `index` holding `text`, which spans `start_offset..end_offset`
    /// of its source.
    pub fn new(
        index: usize,
        text: impl Into<String>,
        start_offset: usize,
        end_offset: usize,
    ) -> Self {
        Self {
            index,
            text: text.into(),
            start_offset,
            end_offset,
        }
    }

    /// Estimated token count of this chunk, from [`estimate_tokens`].
    pub fn token_count(&self) -> usize {
        estimate_tokens(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_count_matches_the_estimate_for_its_text() {
        let chunk = TextChunk::new(0, "Hello world this is a test", 0, 26);

        let tokens = chunk.token_count();
        assert!(tokens > 0);
        assert_eq!(tokens, estimate_tokens(&chunk.text));
    }
}
