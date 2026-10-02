//! Host-owned persistence for chunks.

use crate::error::Result;
use crate::hit::Hit;

/// Host-owned persistence for chunks.
///
/// This crate does not open a database. A host implements [`Store`] against
/// its own rows and hands it to [`index_tree`](crate::index_tree) and
/// [`index_texts`](crate::index_texts). [`Memory`](crate::Memory) is an
/// in-memory implementation.
pub trait Store: Send + Sync {
    /// True when this file is already indexed at `hash`.
    fn file_current(&self, root: &str, relative: &str, hash: &str) -> Result<bool>;

    /// Replace every chunk for `relative` under `root`.
    ///
    /// `chunks` is `(text, optional embedding)` in ordinal order. An empty
    /// slice deletes the file from the index.
    fn replace_file(
        &self,
        root: &str,
        relative: &str,
        language: &str,
        hash: &str,
        chunks: &[(String, Option<Vec<f32>>)],
    ) -> Result<()>;

    /// Rank persisted chunks for `query`.
    fn search(&self, query: &str, root: Option<&str>, limit: usize) -> Result<Vec<Hit>>;

    /// Rank persisted chunks against `vector` by cosine similarity.
    fn search_vector(&self, vector: &[f32], root: Option<&str>, limit: usize) -> Result<Vec<Hit>>;
}
