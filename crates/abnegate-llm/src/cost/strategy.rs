use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::parse_error::ParseError;

const KIND: &str = "cost strategy";
const BUDGET_PREFIX: &str = "budget:";

/// How to choose between the models that can do a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CostStrategy {
    CheapestPossible,
    BestQuality,
    BestValue,
    Budget { max_usd: f64 },
    LocalFirst,
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
        Ok(Self::Budget { max_usd: budget })
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
            CostStrategy::Budget { max_usd: 5.0 },
            CostStrategy::LocalFirst,
        ];

        for strategy in &strategies {
            let json = serde_json::to_string(strategy).unwrap();
            let roundtrip: CostStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(*strategy, roundtrip);
        }
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
            ("budget:25.50", CostStrategy::Budget { max_usd: 25.5 }),
            ("budget:0", CostStrategy::Budget { max_usd: 0.0 }),
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
