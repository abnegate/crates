//! Choosing which provider answers a request.

use sha2::{Digest, Sha256};

use crate::provider::weighted::Weighted;

const MANTISSA_BITS: u32 = 53;

/// Inverse-CDF selection over the weights.
///
/// Weights are shares, not probabilities: they need not sum to one. A negative
/// or non-finite weight is treated as zero rather than allowed to corrupt the
/// running total, and a set with no positive weight at all falls back to the
/// first provider.
pub fn choose(providers: &[Weighted], sample: f64) -> usize {
    if providers.is_empty() {
        return 0;
    }

    let share = |weighted: &Weighted| {
        if weighted.weight.is_finite() && weighted.weight > 0.0 {
            weighted.weight
        } else {
            0.0
        }
    };

    let total: f64 = providers.iter().map(share).sum();
    if total <= 0.0 {
        return 0;
    }

    let sample = if sample.is_finite() {
        sample.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let roll = sample * total;

    let mut cumulative = 0.0;
    for (index, weighted) in providers.iter().enumerate() {
        cumulative += share(weighted);
        if roll < cumulative {
            return index;
        }
    }

    // Only reachable when rounding leaves `roll` at or past the total.
    providers
        .iter()
        .rposition(|weighted| share(weighted) > 0.0)
        .unwrap_or(0)
}

/// A split sample derived from a stable key rather than from entropy.
///
/// Drawing a fresh sample per request re-rolls the experiment on every retry,
/// so one task can be served by the control arm and its retry by the variant.
/// Deriving the sample from the task's own id keeps a task in the arm it
/// started in, which is what makes the two arms comparable afterwards.
pub fn sample(key: &str) -> f64 {
    let digest = Sha256::digest(key.as_bytes());
    let mut leading = [0_u8; 8];
    leading.copy_from_slice(&digest[..8]);

    // 53 bits is every integer an f64 represents exactly, so the division is
    // lossless and the result never rounds up to 1.0.
    let bits = u64::from_be_bytes(leading) >> (u64::BITS - MANTISSA_BITS);
    bits as f64 / (1_u64 << MANTISSA_BITS) as f64
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{choose, sample};
    use crate::provider::testing::StubProvider;
    use crate::provider::weighted::Weighted;

    fn arms(weights: &[f64]) -> Vec<Weighted> {
        weights
            .iter()
            .enumerate()
            .map(|(index, weight)| {
                Weighted::new(
                    Arc::new(StubProvider::answering(format!("arm{index}"), "hello")) as Arc<_>,
                    *weight,
                )
            })
            .collect()
    }

    #[test]
    fn a_seeded_sweep_lands_in_each_arm_in_proportion_to_its_weight() {
        let providers = arms(&[70.0, 20.0, 10.0]);

        let mut counts = [0_usize; 3];
        let samples = 1000;
        for step in 0..samples {
            let sample = f64::from(step) / f64::from(samples);
            counts[choose(&providers, sample)] += 1;
        }

        assert_eq!(counts, [700, 200, 100]);
    }

    #[test]
    fn the_boundary_between_two_arms_falls_where_the_weights_put_it() {
        let providers = arms(&[25.0, 75.0]);

        assert_eq!(choose(&providers, 0.0), 0);
        assert_eq!(choose(&providers, 0.2499), 0);
        assert_eq!(choose(&providers, 0.25), 1);
        assert_eq!(choose(&providers, 0.9999), 1);
    }

    #[test]
    fn the_same_sample_always_lands_in_the_same_arm() {
        let providers = arms(&[1.0, 1.0, 1.0]);

        for sample in [0.0, 0.1, 0.34, 0.5, 0.67, 0.99] {
            let first = choose(&providers, sample);
            assert_eq!(choose(&providers, sample), first, "sample {sample} drifted");
        }
    }

    #[test]
    fn a_zero_weight_arm_is_never_drawn() {
        let providers = arms(&[0.0, 1.0, 0.0]);

        for step in 0..500 {
            let sample = f64::from(step) / 500.0;
            assert_eq!(
                choose(&providers, sample),
                1,
                "sample {sample} drew a spare"
            );
        }
    }

    #[test]
    fn weights_that_cannot_form_a_distribution_fall_back_to_the_first() {
        for weights in [
            vec![0.0, 0.0],
            vec![-1.0, -2.0],
            vec![f64::NAN, f64::NEG_INFINITY],
        ] {
            let providers = arms(&weights);
            assert_eq!(choose(&providers, 0.5), 0, "weights {weights:?}");
        }
    }

    #[test]
    fn a_negative_arm_does_not_shift_the_arms_beside_it() {
        let providers = arms(&[-5.0, 1.0, 1.0]);

        assert_eq!(choose(&providers, 0.0), 1);
        assert_eq!(choose(&providers, 0.49), 1);
        assert_eq!(choose(&providers, 0.5), 2);
    }

    #[test]
    fn a_sample_outside_the_unit_interval_is_clamped_rather_than_wrapped() {
        let providers = arms(&[1.0, 1.0]);

        assert_eq!(choose(&providers, -3.0), 0);
        assert_eq!(choose(&providers, 1.0), 1);
        assert_eq!(choose(&providers, 9.0), 1);
        assert_eq!(choose(&providers, f64::NAN), 0);
    }

    #[test]
    fn an_empty_set_has_nothing_to_choose() {
        assert_eq!(choose(&[], 0.5), 0);
    }

    #[test]
    fn a_lone_arm_is_always_the_one_chosen() {
        let providers = arms(&[1.0]);

        for sample in [0.0, 0.25, 0.5, 0.75, 0.999] {
            assert_eq!(choose(&providers, sample), 0, "sample {sample}");
        }
    }

    #[test]
    fn a_key_always_derives_the_same_sample() {
        let key = "task-018f2c41-0000-7000-8000-000000000001";

        let first = sample(key);
        assert!((0.0..1.0).contains(&first), "sample {first} left the range");
        for _ in 0..8 {
            assert_eq!(sample(key), first, "the bucket drifted");
        }
        assert_ne!(
            sample(key),
            sample("task-018f2c41-0000-7000-8000-000000000002")
        );
    }

    #[test]
    fn a_task_keeps_the_arm_it_started_in_across_retries() {
        let providers = arms(&[50.0, 50.0]);
        let key = "task-018f2c41-0000-7000-8000-00000000000a";

        let first = choose(&providers, sample(key));
        for _ in 0..5 {
            assert_eq!(
                choose(&providers, sample(key)),
                first,
                "the retry switched arms"
            );
        }
    }

    #[test]
    fn keys_spread_across_the_arms_rather_than_piling_into_one() {
        let providers = arms(&[50.0, 50.0]);

        let mut counts = [0_usize; 2];
        for index in 0..1000 {
            counts[choose(&providers, sample(&format!("task-{index}")))] += 1;
        }

        assert!(
            counts[0] > 400 && counts[1] > 400,
            "the split was lopsided: {counts:?}"
        );
    }
}
