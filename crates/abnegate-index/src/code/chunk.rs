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
    /// Start byte offset in the source.
    pub start_offset: usize,
    /// End byte offset in the source.
    pub end_offset: usize,
    /// What the chunk covers.
    pub metadata: ChunkMetadata,
    /// The identifiers the chunk defines and references.
    pub identifiers: IdentifierBag,
}
