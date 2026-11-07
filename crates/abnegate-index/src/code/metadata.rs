//! What a code chunk covers and where it came from.

use crate::code::symbol_kind::SymbolKind;
use crate::code::visibility::Visibility;

/// What a [`CodeChunk`](crate::CodeChunk) covers and where it came from.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ChunkMetadata {
    /// The file path the caller passed in [`ChunkConfig`](crate::ChunkConfig).
    pub path: Option<String>,
    /// The symbol name.
    pub symbol: Option<String>,
    /// What the symbol declares.
    pub kind: Option<SymbolKind>,
    /// How visible the symbol is.
    pub visibility: Visibility,
    /// Names of the enclosing symbols, outermost first.
    pub parents: Vec<String>,
    /// The file's module or package declaration.
    pub module: Option<String>,
    /// Whether the chunk looks like test code.
    pub is_test: bool,
    /// Whether the source looked generated or minified.
    pub is_generated: bool,
    /// About where the chunk starts in the source, in bytes. A split chunk can
    /// report an offset past the end of the source or past `end_byte`, so this
    /// locates a chunk but must not slice the source.
    pub start_byte: usize,
    /// About where the chunk ends in the source, in bytes, with the same caveat
    /// as `start_byte`.
    pub end_byte: usize,
    /// The first 8 bytes of the SHA-256 of the chunk text, as 16 lowercase
    /// hexadecimal characters.
    pub content_hash: String,
}
