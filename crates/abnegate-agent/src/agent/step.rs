use abnegate_llm::Message;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentPhase, ToolCallResult};

/// A step in the agent's execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    /// Unique ID for this step
    pub id: Uuid,
    /// The phase of this step
    pub phase: AgentPhase,
    /// The message that was sent/received
    pub message: Option<Message>,
    /// Tool calls made in this step
    pub tool_calls: Option<Vec<ToolCallResult>>,
    /// Timestamp when this step started
    pub started_at: DateTime<Utc>,
    /// Timestamp when this step completed
    pub completed_at: Option<DateTime<Utc>>,
}

impl AgentStep {
    pub fn new(phase: AgentPhase) -> Self {
        Self {
            id: Uuid::new_v4(),
            phase,
            message: None,
            tool_calls: None,
            started_at: Utc::now(),
            completed_at: None,
        }
    }

    pub fn with_message(mut self, message: Message) -> Self {
        self.message = Some(message);
        self
    }

    pub fn complete(mut self) -> Self {
        self.completed_at = Some(Utc::now());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolCallResult;
    use abnegate_llm::{FunctionCall, ToolCall};

    #[test]
    fn test_agent_step_new() {
        let step = AgentStep::new(AgentPhase::Thinking);
        assert_eq!(step.phase, AgentPhase::Thinking);
        assert!(step.message.is_none());
        assert!(step.tool_calls.is_none());
        assert!(step.completed_at.is_none());
    }

    #[test]
    fn test_agent_step_with_message() {
        let step = AgentStep::new(AgentPhase::Thinking).with_message(Message::user("test"));
        assert!(step.message.is_some());
    }

    #[test]
    fn test_agent_step_complete() {
        let step = AgentStep::new(AgentPhase::Thinking).complete();
        assert!(step.completed_at.is_some());
    }

    #[test]
    fn test_agent_step_with_tool_calls() {
        let tool_call = ToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path": "/tmp/test"}"#.to_string(),
            },
        };

        let tool_result = ToolCallResult {
            call: tool_call,
            result: "file contents".to_string(),
            success: true,
            duration_ms: 150,
        };

        let mut step = AgentStep::new(AgentPhase::Acting);
        step.tool_calls = Some(vec![tool_result]);

        assert!(step.tool_calls.is_some());
        assert_eq!(step.tool_calls.as_ref().unwrap().len(), 1);
        assert!(step.tool_calls.as_ref().unwrap()[0].success);
    }

    #[test]
    fn test_agent_step_serialization_roundtrip() {
        let step = AgentStep::new(AgentPhase::Thinking)
            .with_message(Message::user("test message"))
            .complete();

        let json = serde_json::to_string(&step).unwrap();
        let deserialized: AgentStep = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.phase, AgentPhase::Thinking);
        assert!(deserialized.message.is_some());
        assert!(deserialized.completed_at.is_some());
    }

    #[test]
    fn test_agent_step_id_uniqueness() {
        let step1 = AgentStep::new(AgentPhase::Thinking);
        let step2 = AgentStep::new(AgentPhase::Thinking);

        assert_ne!(step1.id, step2.id);
    }

    #[test]
    fn test_agent_step_timing() {
        let step = AgentStep::new(AgentPhase::Acting);
        let start_time = step.started_at;

        std::thread::sleep(std::time::Duration::from_millis(5));

        let completed_step = step.complete();
        let end_time = completed_step.completed_at.unwrap();

        assert!(end_time > start_time);
    }
}
