//! Vector similarity helpers.

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

/// Euclidean distance of two embedding vectors.
///
/// A pair that does not share a length scores [`f64::MAX`]. Empty equal-length
/// vectors score `0.0`.
pub fn euclidean_distance(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() {
        return f64::MAX;
    }
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| {
            let delta = f64::from(*a) - f64::from(*b);
            delta * delta
        })
        .sum::<f64>()
        .sqrt()
}

/// `vector` scaled to unit length. A zero vector is returned unchanged.
pub fn normalize(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|value| {
            let value = f64::from(*value);
            value * value
        })
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return vector.to_vec();
    }
    vector
        .iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
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

    #[test]
    fn euclidean_distance_matches_the_3_4_5_triangle() {
        assert!((euclidean_distance(&[0.0, 0.0], &[3.0, 4.0]) - 5.0).abs() < 1e-9);
        assert!(euclidean_distance(&[1.0], &[1.0, 2.0]) > 1e9);
        assert!((euclidean_distance(&[], &[])).abs() < 1e-9);
    }

    #[test]
    fn normalize_scales_to_unit_length() {
        let scaled = normalize(&[3.0, 4.0]);
        let norm = scaled.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert_eq!(normalize(&[0.0, 0.0]), vec![0.0, 0.0]);
        assert!(normalize(&[]).is_empty());
    }
}
