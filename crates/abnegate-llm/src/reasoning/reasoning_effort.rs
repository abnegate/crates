use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use crate::parse_error::ParseError;
use crate::reasoning::Effort;
use crate::reasoning::classify;

const KIND: &str = "reasoning effort";

/// User-facing effort. Auto inspects the request; Off skips thinking.
///
/// The levels above [`Self::High`] are for a host whose backend, such as a
/// coding agent, thinks harder than a completions API can be asked to. A
/// completions API receives each of them as [`Effort::High`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasoningEffort {
    /// Pick an effort from the prompt. The default.
    #[default]
    Auto,
    /// Do not think.
    Off,
    /// Always think briefly.
    Low,
    /// Always think moderately.
    Medium,
    /// Always think at length.
    High,
    /// Think longer than [`Self::High`].
    #[serde(rename = "xhigh")]
    XHigh,
    /// Think as long as the backend allows.
    Max,
    /// Think as long as the backend allows, with whatever it adds beyond that.
    Ultra,
}

impl ReasoningEffort {
    /// Every level, from [`Self::Auto`] through [`Self::Ultra`], in the order
    /// a host lists them.
    pub const ALL: &[Self] = &[
        Self::Auto,
        Self::Off,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
        Self::Ultra,
    ];

    /// The levels a host offers for a completions API: [`Self::Off`] through
    /// [`Self::High`]. Anything above `High` reaches one as `High`, the most
    /// it takes.
    pub const HTTP: &[Self] = &[Self::Off, Self::Low, Self::Medium, Self::High];

    /// The name, which is also the serialized form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    /// The effort to send, if thinking should run at all.
    pub fn resolve(self, prompt: &str) -> Option<Effort> {
        match self {
            Self::Off => None,
            Self::Low => Some(Effort::Low),
            Self::Medium => Some(Effort::Medium),
            Self::High | Self::XHigh | Self::Max | Self::Ultra => Some(Effort::High),
            Self::Auto => Some(classify(prompt)),
        }
    }
}

/// Reads the names [`ReasoningEffort::as_str`] writes, exactly.
impl FromStr for ReasoningEffort {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|effort| effort.as_str() == value)
            .ok_or_else(|| ParseError::new(KIND, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 8] = [
        "auto", "off", "low", "medium", "high", "xhigh", "max", "ultra",
    ];

    #[test]
    fn every_effort_round_trips_through_its_name() {
        assert_eq!(ReasoningEffort::ALL.len(), NAMES.len());
        for (effort, name) in ReasoningEffort::ALL.iter().copied().zip(NAMES) {
            assert_eq!(effort.as_str(), name);
            assert_eq!(name.parse::<ReasoningEffort>(), Ok(effort));
        }
    }

    #[test]
    fn only_the_exact_name_parses() {
        for value in ["extreme", "x_high", "XHIGH", "High", " max", "max ", ""] {
            let error = value.parse::<ReasoningEffort>().unwrap_err();
            assert_eq!(error.kind(), KIND, "{value:?}");
        }
    }

    #[test]
    fn serde_spells_every_level_as_its_name() {
        for effort in ReasoningEffort::ALL.iter().copied() {
            let spelled = serde_json::to_value(effort).expect("an effort to serialize");
            assert_eq!(spelled, serde_json::json!(effort.as_str()));
            assert_eq!(
                serde_json::from_value::<ReasoningEffort>(spelled).expect("an effort to read"),
                effort
            );
        }
    }

    #[test]
    fn levels_above_high_reach_a_completions_api_as_high() {
        for effort in [
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
            ReasoningEffort::Ultra,
        ] {
            assert_eq!(effort.resolve("Hi"), Some(Effort::High), "{effort:?}");
            assert!(!ReasoningEffort::HTTP.contains(&effort), "{effort:?}");
        }
    }

    #[test]
    fn a_completions_api_is_offered_off_through_high() {
        assert_eq!(
            ReasoningEffort::HTTP,
            [
                ReasoningEffort::Off,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        );
    }
}
