//! A linear ranker over [`RankFeatures`] and its pairwise training.

use crate::rank::features::FEATURE_COUNT;
use crate::rank::features::RankFeatures;

/// A linear model over [`RankFeatures`]: a bias plus one weight per feature.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LinearRanker {
    /// One weight per feature, in [`extract_features`](crate::extract_features)
    /// order.
    pub weights: [f32; FEATURE_COUNT],
    /// Added to every score.
    pub bias: f32,
}

impl LinearRanker {
    /// A ranker with these `weights` and `bias`.
    pub fn new(weights: [f32; FEATURE_COUNT], bias: f32) -> Self {
        Self { weights, bias }
    }

    /// The bias plus the dot product of the weights with `features`, summed in
    /// feature order.
    pub fn score(&self, features: &RankFeatures) -> f32 {
        let mut dot = self.bias;
        for (weight, value) in self.weights.iter().zip(features.values.iter()) {
            dot += weight * value;
        }
        dot
    }
}

/// Fit a [`LinearRanker`] to `(better, worse)` feature pairs with RankNet
/// pairwise logistic loss.
///
/// Runs `steps` full-batch gradient steps of size `learning_rate` from zero
/// weights, with L2 decay of 0.002 on the weights and a bias gradient scaled
/// by 0.05. No pairs yield the zero ranker.
pub fn fit_pairwise(
    pairs: &[(RankFeatures, RankFeatures)],
    steps: usize,
    learning_rate: f32,
) -> LinearRanker {
    let mut ranker = LinearRanker::new([0.0; FEATURE_COUNT], 0.0);
    if pairs.is_empty() {
        return ranker;
    }
    for _ in 0..steps {
        let mut weight_gradients = [0.0f32; FEATURE_COUNT];
        let mut bias_gradient = 0.0f32;
        for (positive, negative) in pairs {
            let positive_score = ranker.score(positive);
            let negative_score = ranker.score(negative);
            let error = sigmoid(negative_score - positive_score);
            for (gradient, (positive_value, negative_value)) in weight_gradients
                .iter_mut()
                .zip(positive.values.iter().zip(negative.values.iter()))
            {
                *gradient += error * (*positive_value - *negative_value);
            }
            bias_gradient += error * 0.05;
        }
        let count = pairs.len() as f32;
        for (weight, gradient) in ranker.weights.iter_mut().zip(weight_gradients.iter()) {
            let decay = 0.002 * *weight;
            *weight += learning_rate * (*gradient / count - decay);
        }
        ranker.bias += learning_rate * (bias_gradient / count);
    }
    ranker
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value.clamp(-20.0, 20.0)).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rank::features::extract_features;

    #[test]
    fn pairwise_fit_separates_a_known_pair() {
        let positive = extract_features(
            "align_vector",
            "pgvector.rs",
            "pgvector.rs",
            "fn align_vector pads shorter embeddings",
            Some(0.5),
            Some(0.4),
            Some(1),
            Some(3),
            0.016,
        );
        let negative = extract_features(
            "align_vector",
            "auth.ts",
            "auth.ts",
            "login form",
            Some(0.9),
            Some(0.01),
            None,
            Some(1),
            0.012,
        );
        let ranker = fit_pairwise(&[(positive.clone(), negative.clone())], 80, 0.4);
        assert!(ranker.score(&positive) > ranker.score(&negative));
    }

    #[test]
    fn no_pairs_fit_the_zero_ranker() {
        let ranker = fit_pairwise(&[], 10, 0.4);
        assert_eq!(ranker.weights, [0.0; FEATURE_COUNT]);
        assert_eq!(ranker.bias, 0.0);
    }

    #[test]
    fn score_adds_the_bias_to_the_dot_product() {
        let mut weights = [0.0; FEATURE_COUNT];
        weights[0] = 2.0;
        weights[3] = -1.0;
        let mut values = [0.0; FEATURE_COUNT];
        values[0] = 0.5;
        values[3] = 0.25;
        let ranker = LinearRanker::new(weights, 0.5);
        assert_eq!(ranker.score(&RankFeatures::new(values)), 1.25);
    }
}
