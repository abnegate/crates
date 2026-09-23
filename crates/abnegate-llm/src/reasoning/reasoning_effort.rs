use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::parse_error::ParseError;
use crate::reasoning::{Effort, classify};

const KIND: &str = "reasoning effort";

/// User-facing effort. Auto inspects the request; Off skips thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasoningEffort {
    #[default]
    Auto,
    Off,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// The effort to send, if thinking should run at all.
    pub fn resolve(self, prompt: &str) -> Option<Effort> {
        match self {
            Self::Off => None,
            Self::Low => Some(Effort::Low),
            Self::Medium => Some(Effort::Medium),
            Self::High => Some(Effort::High),
            Self::Auto => Some(classify(prompt)),
        }
    }
}

/// Reads the names [`ReasoningEffort::as_str`] writes.
impl FromStr for ReasoningEffort {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err(ParseError::new(KIND, value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effort_round_trips_through_its_name() {
        for effort in [
            ReasoningEffort::Auto,
            ReasoningEffort::Off,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ] {
            assert_eq!(effort.as_str().parse::<ReasoningEffort>(), Ok(effort));
        }
        let error = "extreme".parse::<ReasoningEffort>().unwrap_err();
        assert_eq!(error.kind(), KIND);
    }
}
