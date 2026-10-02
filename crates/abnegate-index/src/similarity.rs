//! Vector similarity helpers.

/// Lowest cosine score a host typically keeps as a neighbour.
pub const SIMILARITY_FLOOR: f64 = 0.7;

/// Cosine similarity of two embedding vectors, in `-1.0..=1.0`.
///
/// A pair that does not share a length, or that is empty, scores `0.0`.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for index in 0..left.len() {
        let a = f64::from(left[index]);
        let b = f64::from(right[index]);
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_score_one() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn opposite_vectors_score_minus_one() {
        assert!((cosine_similarity(&[1.0, 2.0, 3.0], &[-1.0, -2.0, -3.0]) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn mismatched_lengths_score_zero() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }
}
