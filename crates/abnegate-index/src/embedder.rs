//! Host-owned embeddings for chunk text.

use crate::error::Result;

/// Host-owned embeddings for chunk text.
///
/// This crate never loads a model. A host implements [`Embedder`] against its
/// own provider and hands it to [`index_tree`](crate::index_tree) and
/// [`index_texts`](crate::index_texts).
pub trait Embedder: Send + Sync {
    /// One embedding for each of `texts`, in the same order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}
