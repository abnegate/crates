//! Semantic category match against reference embeddings.

use crate::similarity::cosine_similarity;

/// A named reference embedding used to classify an error or skip reason.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Category {
    /// Category name, such as `timeout` or `not_found`.
    pub name: String,
    /// Embedding of a description of that category.
    pub embedding: Vec<f32>,
}

impl Category {
    /// A category named `name` with `embedding`.
    pub fn new(name: impl Into<String>, embedding: Vec<f32>) -> Self {
        Self {
            name: name.into(),
            embedding,
        }
    }

    /// The closest category to `embedding` whose cosine score is at least
    /// `threshold`. `None` when nothing clears the floor, or when `embedding`
    /// is empty.
    pub fn nearest<'a>(
        embedding: &[f32],
        categories: &'a [Self],
        threshold: f64,
    ) -> Option<&'a Self> {
        if embedding.is_empty() {
            return None;
        }
        let mut best: Option<(&Category, f64)> = None;
        for category in categories {
            let score = cosine_similarity(embedding, &category.embedding);
            if score < threshold {
                continue;
            }
            match best {
                Some((_, best_score)) if score <= best_score => {}
                _ => best = Some((category, score)),
            }
        }
        best.map(|(category, _)| category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_picks_the_matching_reference() {
        let categories = [
            Category::new("timeout", vec![1.0, 0.0]),
            Category::new("syntax", vec![0.0, 1.0]),
        ];
        let hit = Category::nearest(&[0.95, 0.05], &categories, 0.3).unwrap();
        assert_eq!(hit.name, "timeout");
        assert!(Category::nearest(&[0.5, 0.5], &categories, 0.99).is_none());
    }
}
