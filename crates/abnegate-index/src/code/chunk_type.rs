//! The view of the source a code chunk holds.

/// The view of the source a [`CodeChunk`](crate::CodeChunk) holds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodeChunkType {
    /// A full symbol body with its header, docstring, imports and identifiers.
    Body,
    /// A symbol's signature, docstring and key identifiers, without its body.
    Skeleton,
    /// The file's leading docstring, module declaration and imports.
    FileHeader,
    /// The signatures of every public symbol in the file.
    ApiSurface,
    /// Top-level source no symbol covers, or a plain text window.
    TopLevel,
}
