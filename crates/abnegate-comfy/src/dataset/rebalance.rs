use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

/// How often the trainer draws each frame of a person's set, so one pose
/// that dominates the photos does not dominate the run, as
/// [`rebalance`](crate::dataset::rebalance) works it out and
/// [`REBALANCE_FILE`](crate::host::REBALANCE_FILE) holds it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Rebalance {
    /// How many frames show each pose cluster.
    pub clusters: BTreeMap<String, usize>,
    /// Each frame's draw weight, keyed by its stem, between 0 and 1.
    pub weights: BTreeMap<String, f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rebalance_keeps_the_wire_names_the_trainer_reads() {
        let rebalance = Rebalance {
            clusters: BTreeMap::from([("standing".to_string(), 2)]),
            weights: BTreeMap::from([("0000".to_string(), 0.5)]),
        };
        let encoded = serde_json::to_value(&rebalance).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"clusters": {"standing": 2}, "weights": {"0000": 0.5}})
        );
        assert_eq!(
            serde_json::from_value::<Rebalance>(encoded).unwrap(),
            rebalance
        );
    }
}
