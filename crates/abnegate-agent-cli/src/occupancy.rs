//! How full a coding agent's context was when its turn ended.

use serde::Deserialize;
use serde::Serialize;

/// The context window of the model an agent ran its turn on, and how many
/// tokens of it the turn's last request filled, both as the agent reported
/// them rather than as a caller would estimate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Occupancy {
    /// The model's context window, in tokens.
    pub window: u64,
    /// The tokens the turn's last request filled, when the agent said.
    pub used: Option<u64>,
}

impl Occupancy {
    /// `window` and `used` as an agent reported them, or `None` when it
    /// reported no window. A count of zero is a count left unreported.
    pub fn reported(window: u64, used: Option<u64>) -> Option<Self> {
        (window > 0).then(|| Self {
            window,
            used: used.filter(|used| *used > 0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Occupancy;

    #[test]
    fn a_window_is_needed_and_zero_counts_as_unreported() {
        assert_eq!(Occupancy::reported(0, Some(10)), None);
        assert_eq!(
            Occupancy::reported(200_000, Some(0)),
            Some(Occupancy {
                window: 200_000,
                used: None,
            })
        );
        assert_eq!(
            Occupancy::reported(200_000, Some(18_400)),
            Some(Occupancy {
                window: 200_000,
                used: Some(18_400),
            })
        );
        assert_eq!(
            Occupancy::reported(200_000, None).map(|occupancy| occupancy.used),
            Some(None)
        );
    }

    #[test]
    fn it_round_trips_through_json() {
        let occupancy = Occupancy::reported(475_000, Some(24_638)).expect("an occupancy");
        let json = serde_json::to_value(occupancy).expect("it serialises");

        assert_eq!(json, serde_json::json!({"window": 475_000, "used": 24_638}));
        assert_eq!(
            serde_json::from_value::<Occupancy>(json).expect("it deserialises"),
            occupancy
        );
    }
}
