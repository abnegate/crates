//! A chunk of source code with its context.

use crate::code::chunk_type::CodeChunkType;
use crate::code::identifier_bag::IdentifierBag;
use crate::code::metadata::ChunkMetadata;

/// A chunk of source code, its header and identifiers, ready to embed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CodeChunk {
    /// Position of this chunk in the file's chunk list.
    pub index: usize,
    /// The view of the source this chunk holds.
    pub chunk_type: CodeChunkType,
    /// The chunk text, header included.
    pub text: String,
    /// About where the chunk starts in the source, in bytes. A split chunk can
    /// report an offset past the end of the source or past `end_offset`, so
    /// this locates a chunk but must not slice the source.
    pub start_offset: usize,
    /// About where the chunk ends in the source, in bytes, with the same
    /// caveat as `start_offset`.
    pub end_offset: usize,
    /// What the chunk covers.
    pub metadata: ChunkMetadata,
    /// The identifiers the chunk defines and references.
    pub identifiers: IdentifierBag,
}

impl CodeChunk {
    /// A chunk at position `index` with no metadata or identifiers.
    pub fn new(
        index: usize,
        chunk_type: CodeChunkType,
        text: impl Into<String>,
        start_offset: usize,
        end_offset: usize,
    ) -> Self {
        Self {
            index,
            chunk_type,
            text: text.into(),
            start_offset,
            end_offset,
            metadata: ChunkMetadata::default(),
            identifiers: IdentifierBag::default(),
        }
    }

    /// Set what the chunk covers.
    pub fn with_metadata(mut self, metadata: ChunkMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the identifiers the chunk defines and references.
    pub fn with_identifiers(mut self, identifiers: IdentifierBag) -> Self {
        self.identifiers = identifiers;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_with_empty_metadata_and_identifiers() {
        let chunk = CodeChunk::new(3, CodeChunkType::Body, "fn run() {}", 10, 21);

        assert_eq!(chunk.index, 3);
        assert_eq!(chunk.chunk_type, CodeChunkType::Body);
        assert_eq!(chunk.text, "fn run() {}");
        assert_eq!((chunk.start_offset, chunk.end_offset), (10, 21));
        assert_eq!(chunk.metadata.content_hash, "");
        assert!(chunk.identifiers.defined_symbol.is_none());
    }

    #[test]
    fn builders_replace_the_fields_they_name() {
        let chunk = CodeChunk::new(0, CodeChunkType::Body, "", 0, 0)
            .with_metadata(ChunkMetadata {
                is_test: true,
                ..ChunkMetadata::default()
            })
            .with_identifiers(IdentifierBag {
                key_strings: vec!["key".to_string()],
                ..IdentifierBag::default()
            });

        assert!(chunk.metadata.is_test);
        assert_eq!(chunk.identifiers.key_strings, ["key"]);
    }
}
