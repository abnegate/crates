//! Host-owned embeddings for trial text.

use crate::error::Result;

/// Host-owned embeddings for trial text.
///
/// This crate never loads a model. A host implements [`Embedder`] against its
/// own provider and hands it to
/// [`Memory::record_embedded`](crate::Memory::record_embedded) and
/// [`Memory::embed_missing`](crate::Memory::embed_missing).
pub trait Embedder: Send + Sync {
    /// One embedding for each of `texts`, in the same order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::memory::Memory;
    use crate::trial_input::TrialInput;
    use crate::verdict::Verdict;

    struct Scripted {
        vector: Vec<f32>,
    }

    impl Embedder for Scripted {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Err(Error::embed("no texts"));
            }
            Ok(texts.iter().map(|_| self.vector.clone()).collect())
        }
    }

    #[test]
    fn record_embedded_fills_a_missing_vector() {
        let mut memory = Memory::new();
        let id = memory
            .record_embedded(
                TrialInput::new("lab", "fuzz").with_verdict(Verdict::Skip),
                &Scripted {
                    vector: vec![1.0, 0.0],
                },
            )
            .unwrap();
        assert_eq!(
            memory.get(id).unwrap().embedding.as_deref(),
            Some(&[1.0, 0.0][..])
        );
    }

    #[test]
    fn embed_missing_fills_only_trials_without_a_vector() {
        let mut memory = Memory::new();
        let with = memory.record(
            TrialInput::new("lab", "fuzz")
                .with_verdict(Verdict::Skip)
                .with_embedding(vec![0.0, 1.0]),
        );
        let without = memory.record(TrialInput::new("lab", "jit").with_verdict(Verdict::Empty));
        let filled = memory
            .embed_missing(&Scripted {
                vector: vec![1.0, 0.0],
            })
            .unwrap();
        assert_eq!(filled, 1);
        assert_eq!(
            memory.get(with).unwrap().embedding.as_deref(),
            Some(&[0.0, 1.0][..])
        );
        assert_eq!(
            memory.get(without).unwrap().embedding.as_deref(),
            Some(&[1.0, 0.0][..])
        );
    }
}
