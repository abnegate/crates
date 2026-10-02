//! Test double that returns inserted vectors.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::embedder::Embedder;
use crate::error::Result;

/// Test double that returns inserted vectors, or a stable hash vector.
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
#[derive(Debug)]
pub struct Scripted {
    dimensions: usize,
    vectors: Mutex<HashMap<String, Vec<f32>>>,
    needles: Mutex<Vec<(String, Vec<f32>)>>,
}

impl Scripted {
    /// A scripted embedder that hashes unknown text into `dimensions`.
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions: dimensions.max(1),
            vectors: Mutex::new(HashMap::new()),
            needles: Mutex::new(Vec::new()),
        }
    }

    /// Exact `text` returns `vector`.
    pub fn insert(&self, text: impl Into<String>, vector: Vec<f32>) {
        self.vectors
            .lock()
            .expect("embedder")
            .insert(text.into(), vector);
    }

    /// Any embed text that contains `needle` gets `vector` when no exact
    /// insert matches. First matching needle wins.
    pub fn insert_containing(&self, needle: impl Into<String>, vector: Vec<f32>) {
        self.needles
            .lock()
            .expect("embedder")
            .push((needle.into(), vector));
    }
}

impl Embedder for Scripted {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let map = self.vectors.lock().expect("embedder");
        let needles = self.needles.lock().expect("embedder");
        Ok(texts
            .iter()
            .map(|text| {
                if let Some(vector) = map.get(*text) {
                    return vector.clone();
                }
                needles
                    .iter()
                    .find(|(needle, _)| !needle.is_empty() && text.contains(needle.as_str()))
                    .map(|(_, vector)| vector.clone())
                    .unwrap_or_else(|| hash_vector(text, self.dimensions))
            })
            .collect())
    }
}

fn hash_vector(text: &str, dimensions: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimensions];
    for (index, byte) in text.bytes().enumerate() {
        let slot = index % dimensions;
        vector[slot] += f32::from(byte) / 255.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_insert_wins_over_the_hash() {
        let embedder = Scripted::new(2);
        embedder.insert("hello", vec![0.0, 1.0]);
        assert_eq!(embedder.embed(&["hello"]).unwrap()[0], vec![0.0, 1.0]);
        assert_ne!(embedder.embed(&["other"]).unwrap()[0], vec![0.0, 1.0]);
    }

    #[test]
    fn containing_needle_ranks_a_longer_blob() {
        let embedder = Scripted::new(4);
        embedder.insert_containing("get ':id/entities'", vec![1.0, 0.0, 0.0, 0.0]);
        embedder.insert_containing("get ':id/secure_files'", vec![0.98, 0.1, 0.0, 0.0]);
        let vectors = embedder
            .embed(&[
                "class API\n  get ':id/entities' do\n    present entities\n",
                "class API\n  get ':id/secure_files' do\n    present files\n",
                "class API::Version\n  get '/version'\n",
            ])
            .expect("embed");
        assert_eq!(vectors[0], vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(vectors[1], vec![0.98, 0.1, 0.0, 0.0]);
        assert_ne!(vectors[2], vectors[0]);
        assert_ne!(vectors[2], vectors[1]);
    }
}
