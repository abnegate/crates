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

impl ChunkMetadata {
    /// Set the file path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the symbol name.
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Set what the symbol declares.
    pub fn with_kind(mut self, kind: SymbolKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Set how visible the symbol is.
    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Set the names of the enclosing symbols, outermost first.
    pub fn with_parents(mut self, parents: Vec<String>) -> Self {
        self.parents = parents;
        self
    }

    /// Set the file's module or package declaration.
    pub fn with_module(mut self, module: impl Into<String>) -> Self {
        self.module = Some(module.into());
        self
    }

    /// Set whether the chunk looks like test code.
    pub fn with_is_test(mut self, is_test: bool) -> Self {
        self.is_test = is_test;
        self
    }

    /// Set whether the source looked generated or minified.
    pub fn with_is_generated(mut self, is_generated: bool) -> Self {
        self.is_generated = is_generated;
        self
    }

    /// Set about where the chunk starts in the source, in bytes.
    pub fn with_start_byte(mut self, start_byte: usize) -> Self {
        self.start_byte = start_byte;
        self
    }

    /// Set about where the chunk ends in the source, in bytes.
    pub fn with_end_byte(mut self, end_byte: usize) -> Self {
        self.end_byte = end_byte;
        self
    }

    /// Set the content hash, which should be the first 8 bytes of the SHA-256 of
    /// the chunk text, as 16 lowercase hexadecimal characters.
    pub fn with_content_hash(mut self, content_hash: impl Into<String>) -> Self {
        self.content_hash = content_hash.into();
        self
    }
}
