use serde::{Deserialize, Serialize};
use std::fmt;

/// Current phase of the agent execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentPhase {
    /// Initial state, processing user input
    Thinking,
    /// Executing a tool
    Acting,
    /// Waiting for tool results
    Observing,
    /// Formulating final response
    Responding,
    /// Agent has completed
    Complete,
    /// Agent encountered an error
    Error,
}

impl fmt::Display for AgentPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentPhase::Thinking => write!(formatter, "thinking"),
            AgentPhase::Acting => write!(formatter, "acting"),
            AgentPhase::Observing => write!(formatter, "observing"),
            AgentPhase::Responding => write!(formatter, "responding"),
            AgentPhase::Complete => write!(formatter, "complete"),
            AgentPhase::Error => write!(formatter, "error"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_phase_display() {
        assert_eq!(AgentPhase::Thinking.to_string(), "thinking");
        assert_eq!(AgentPhase::Acting.to_string(), "acting");
        assert_eq!(AgentPhase::Observing.to_string(), "observing");
        assert_eq!(AgentPhase::Responding.to_string(), "responding");
        assert_eq!(AgentPhase::Complete.to_string(), "complete");
        assert_eq!(AgentPhase::Error.to_string(), "error");
    }

    #[test]
    fn test_agent_phase_serialization_roundtrip() {
        let phases = vec![
            AgentPhase::Thinking,
            AgentPhase::Acting,
            AgentPhase::Observing,
            AgentPhase::Responding,
            AgentPhase::Complete,
            AgentPhase::Error,
        ];

        for phase in phases {
            let json = serde_json::to_string(&phase).unwrap();
            let deserialized: AgentPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, phase);
        }
    }

    #[test]
    fn test_agent_phase_copy() {
        let phase = AgentPhase::Acting;
        let copied = phase;
        assert_eq!(phase, copied);

        let phase2 = AgentPhase::Observing;
        let copied2 = phase2;
        assert_eq!(phase2, copied2);
    }

    #[test]
    fn test_agent_phase_equality() {
        assert_eq!(AgentPhase::Thinking, AgentPhase::Thinking);
        assert_ne!(AgentPhase::Thinking, AgentPhase::Acting);
        assert_ne!(AgentPhase::Complete, AgentPhase::Error);
    }
}
