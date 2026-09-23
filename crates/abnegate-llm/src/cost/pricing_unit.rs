use serde::{Deserialize, Serialize};

/// What a model's price is quoted per.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingUnit {
    PerMillionTokens,
    PerImage,
    PerSecondAudio,
    PerCharacter,
    Per3DModel,
    PerVideoSecond,
    Free,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips() {
        let units = [
            PricingUnit::PerMillionTokens,
            PricingUnit::PerImage,
            PricingUnit::PerSecondAudio,
            PricingUnit::PerCharacter,
            PricingUnit::Per3DModel,
            PricingUnit::PerVideoSecond,
            PricingUnit::Free,
        ];

        for unit in &units {
            let json = serde_json::to_string(unit).unwrap();
            let roundtrip: PricingUnit = serde_json::from_str(&json).unwrap();
            assert_eq!(*unit, roundtrip);
        }
    }
}
