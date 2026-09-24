use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::parse_error::ParseError;

const KIND: &str = "cost strategy";
const BUDGET_PREFIX: &str = "budget:";

/// How to choose between the models that can do a task.
///
/// [`CostStrategy::Budget`] may gain a field in a minor release, so it is
/// built with [`CostStrategy::budget`] and a pattern outside this crate ends
/// in `..`:
///
/// ```compile_fail,E0639
/// let strategy = abnegate_llm::CostStrategy::Budget { maximum_usd: 5.0 };
/// # let _ = strategy;
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CostStrategy {
    CheapestPossible,
    BestQuality,
    BestValue,
    /// Best value while the whole batch costs at most `maximum_usd` dollars;
    /// past that, each task gets the best model the rest of the budget
    /// affords, or else the cheapest local one. Serialised as `max_usd`.
    #[non_exhaustive]
    Budget {
        #[serde(rename = "max_usd")]
        maximum_usd: f64,
    },
    LocalFirst,
}

impl CostStrategy {
    /// [`Self::Budget`], spending at most `maximum_usd` dollars on the batch.
    pub fn budget(maximum_usd: f64) -> Self {
        Self::Budget { maximum_usd }
    }
}

/// Reads `cheapest`, `best-quality`, `best-value`, `local-first` (or their
/// short and long spellings) and `budget:<dollars>`, where the budget must be
/// a finite amount of zero or more.
impl FromStr for CostStrategy {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cheapest" | "cheapest-possible" => return Ok(Self::CheapestPossible),
            "best-quality" | "quality" => return Ok(Self::BestQuality),
            "best-value" | "value" => return Ok(Self::BestValue),
            "local-first" | "local" => return Ok(Self::LocalFirst),
            _ => {}
        }

        let budget = value
            .strip_prefix(BUDGET_PREFIX)
            .and_then(|amount| amount.trim().parse::<f64>().ok())
            .filter(|amount| amount.is_finite() && *amount >= 0.0)
            .ok_or_else(|| ParseError::new(KIND, value))?;
        Ok(Self::budget(budget))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips() {
        let strategies = [
            CostStrategy::CheapestPossible,
            CostStrategy::BestQuality,
            CostStrategy::BestValue,
            CostStrategy::budget(5.0),
            CostStrategy::LocalFirst,
        ];

        for strategy in &strategies {
            let json = serde_json::to_string(strategy).unwrap();
            let roundtrip: CostStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(*strategy, roundtrip);
        }
    }

    #[test]
    fn a_budget_keeps_its_serialised_name() {
        let budget = CostStrategy::budget(5.0);

        assert_eq!(
            serde_json::to_value(&budget).unwrap(),
            serde_json::json!({ "Budget": { "max_usd": 5.0 } })
        );
        assert_eq!(
            serde_json::from_str::<CostStrategy>(r#"{"Budget":{"max_usd":5.0}}"#).unwrap(),
            budget
        );
    }

    #[test]
    fn every_named_strategy_parses() {
        for (name, expected) in [
            ("cheapest", CostStrategy::CheapestPossible),
            ("cheapest-possible", CostStrategy::CheapestPossible),
            ("best-quality", CostStrategy::BestQuality),
            ("quality", CostStrategy::BestQuality),
            ("best-value", CostStrategy::BestValue),
            ("value", CostStrategy::BestValue),
            ("local-first", CostStrategy::LocalFirst),
            ("local", CostStrategy::LocalFirst),
            ("budget:25.50", CostStrategy::budget(25.5)),
            ("budget:0", CostStrategy::budget(0.0)),
        ] {
            assert_eq!(name.parse::<CostStrategy>(), Ok(expected), "{name}");
        }
    }

    #[test]
    fn a_budget_that_is_not_a_finite_amount_is_refused() {
        for value in [
            "budget:NaN",
            "budget:nan",
            "budget:-5",
            "budget:inf",
            "budget:lots",
            "budget:",
            "priciest",
        ] {
            assert!(value.parse::<CostStrategy>().is_err(), "{value}");
        }
    }
}
