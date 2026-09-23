use std::str::FromStr;

use crate::parse_error::ParseError;
use crate::provider::selection::choose;
use crate::provider::weighted::Weighted;

const KIND: &str = "selection strategy";

/// How a [`Router`](super::Router) picks the provider it starts with, and
/// whether the rest of the list backs that provider up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionStrategy {
    /// The first provider, and only the first.
    #[default]
    Primary,
    /// One provider drawn by weight, and only that one.
    Weighted,
    /// Every provider in order until one answers.
    Fallback,
    /// One provider drawn by weight, then the rest in order behind it.
    WeightedFallback,
}

impl SelectionStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Weighted => "weighted",
            Self::Fallback => "fallback",
            Self::WeightedFallback => "weighted_fallback",
        }
    }

    /// Whether a failure moves on to the next provider.
    pub fn chains(self) -> bool {
        matches!(self, Self::Fallback | Self::WeightedFallback)
    }

    /// The index this strategy starts at, given a sample in `[0, 1)`.
    ///
    /// The sample is a parameter rather than something drawn here so the split
    /// is reproducible: a test can walk the whole distribution, and a caller
    /// that wants an experiment bucket to stick to one task can derive the
    /// sample from that task instead of from entropy.
    pub fn start(self, providers: &[Weighted], sample: f64) -> usize {
        match self {
            Self::Primary | Self::Fallback => 0,
            Self::Weighted | Self::WeightedFallback => choose(providers, sample),
        }
    }
}

/// Reads the names [`SelectionStrategy::as_str`] writes, and
/// `weighted_random` as a spelling of [`SelectionStrategy::Weighted`].
///
/// An unknown name is an error. A caller whose experiment must still serve
/// traffic when its strategy was typed wrong falls back with
/// `.unwrap_or_default()`, which is [`SelectionStrategy::Primary`]: serving
/// from the primary provider is the choice that changes nothing.
impl FromStr for SelectionStrategy {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "primary" => Ok(Self::Primary),
            "weighted" | "weighted_random" => Ok(Self::Weighted),
            "fallback" => Ok(Self::Fallback),
            "weighted_fallback" => Ok(Self::WeightedFallback),
            _ => Err(ParseError::new(KIND, value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::provider::testing::StubProvider;

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
    fn only_the_chaining_strategies_move_past_a_failure() {
        assert!(!SelectionStrategy::Primary.chains());
        assert!(!SelectionStrategy::Weighted.chains());
        assert!(SelectionStrategy::Fallback.chains());
        assert!(SelectionStrategy::WeightedFallback.chains());
    }

    #[test]
    fn the_unweighted_strategies_always_start_at_the_primary() {
        let providers = arms(&[0.0, 100.0]);

        for strategy in [SelectionStrategy::Primary, SelectionStrategy::Fallback] {
            for sample in [0.0, 0.5, 0.99] {
                assert_eq!(strategy.start(&providers, sample), 0, "{strategy:?}");
            }
        }
    }

    #[test]
    fn the_weighted_strategies_start_where_the_sample_points() {
        let providers = arms(&[0.0, 100.0]);

        for strategy in [
            SelectionStrategy::Weighted,
            SelectionStrategy::WeightedFallback,
        ] {
            assert_eq!(strategy.start(&providers, 0.5), 1, "{strategy:?}");
        }
    }

    #[test]
    fn every_strategy_round_trips_through_its_configured_name() {
        for strategy in [
            SelectionStrategy::Primary,
            SelectionStrategy::Weighted,
            SelectionStrategy::Fallback,
            SelectionStrategy::WeightedFallback,
        ] {
            assert_eq!(strategy.as_str().parse(), Ok(strategy));
        }
    }

    #[test]
    fn weighted_random_is_accepted_as_a_spelling_of_weighted() {
        assert_eq!("weighted_random".parse(), Ok(SelectionStrategy::Weighted));
    }

    #[test]
    fn a_strategy_nobody_knows_is_refused_and_defaults_to_the_primary() {
        for value in ["", "unknown", "round_robin", "Primary", "WEIGHTED"] {
            let error = value.parse::<SelectionStrategy>().unwrap_err();
            assert_eq!(error.value(), value);
            assert_eq!(
                value.parse::<SelectionStrategy>().unwrap_or_default(),
                SelectionStrategy::Primary
            );
        }
    }

    #[test]
    fn a_strategy_is_copied_rather_than_moved() {
        let strategy = SelectionStrategy::WeightedFallback;
        let copied = strategy;

        assert_eq!(strategy, copied);
        assert!(format!("{strategy:?}").contains("WeightedFallback"));
    }
}
