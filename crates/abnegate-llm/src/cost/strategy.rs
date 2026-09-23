use serde::{Deserialize, Serialize};

/// How to choose between the models that can do a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CostStrategy {
    CheapestPossible,
    BestQuality,
    BestValue,
    Budget { max_usd: f64 },
    LocalFirst,
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
}
