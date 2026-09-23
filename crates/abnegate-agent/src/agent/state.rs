use abnegate_llm::Message;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentPhase, AgentStep};
use crate::context::Summary;

/// The current state of an agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Unique ID for this execution
    pub id: Uuid,
    /// Current phase
    pub phase: AgentPhase,
    /// All messages in the conversation
    #[serde(with = "abnegate_llm::history")]
    pub messages: Vec<Message>,
    /// Separate checkpoint; canonical messages are append-only.
    #[serde(default)]
    pub summary: Option<Summary>,
    /// Prefix already presented to the model. Newly appended tool output is protected.
    #[serde(default)]
    pub consumed: usize,
    /// All steps taken
    pub steps: Vec<AgentStep>,
    /// Current iteration count
    pub iteration: usize,
    /// Total tokens used
    pub tokens_used: u32,
    /// Whether the agent has finished
    pub finished: bool,
    /// Final response (if finished)
    pub final_response: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// When the execution started
    pub started_at: DateTime<Utc>,
    /// When the execution finished
    pub finished_at: Option<DateTime<Utc>>,
}

impl AgentState {
    /// Create a new agent state with an initial user message
    pub fn new(user_message: impl Into<String>, system_prompt: Option<String>) -> Self {
        let mut messages = Vec::new();

        if let Some(prompt) = system_prompt {
            messages.push(Message::system(prompt));
        }

        messages.push(Message::user(user_message));

        Self {
            id: Uuid::new_v4(),
            phase: AgentPhase::Thinking,
            messages,
            summary: None,
            consumed: 0,
            steps: Vec::new(),
            iteration: 0,
            tokens_used: 0,
            finished: false,
            final_response: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    /// Add a step to the execution
    pub fn add_step(&mut self, step: AgentStep) {
        self.steps.push(step);
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Mark the agent as complete with a response
    pub fn complete(&mut self, response: impl Into<String>) {
        self.phase = AgentPhase::Complete;
        self.finished = true;
        self.final_response = Some(response.into());
        self.finished_at = Some(Utc::now());
    }

    /// Mark the agent as failed with an error
    pub fn fail(&mut self, error: impl Into<String>) {
        self.phase = AgentPhase::Error;
        self.finished = true;
        self.error = Some(error.into());
        self.finished_at = Some(Utc::now());
    }

    /// Get the progress percentage (based on iterations)
    pub fn progress_percent(&self, max_iterations: usize) -> u8 {
        if self.finished {
            return 100;
        }
        if max_iterations == 0 {
            return 0;
        }
        ((self.iteration as f32 / max_iterations as f32) * 100.0).min(99.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolCallResult;
    use abnegate_llm::{FunctionCall, Role, ToolCall};

    #[test]
    fn test_agent_state_new() {
        let state = AgentState::new("Hello", None);
        assert_eq!(state.phase, AgentPhase::Thinking);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.iteration, 0);
        assert!(!state.finished);
    }

    #[test]
    fn test_agent_state_new_with_system_prompt() {
        let state = AgentState::new("Hello", Some("You are helpful".to_string()));
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].role, Role::System);
        assert_eq!(state.messages[1].role, Role::User);
    }

    #[test]
    fn test_agent_state_add_message() {
        let mut state = AgentState::new("Hello", None);
        state.add_message(Message::assistant("Hi there!"));
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn test_agent_state_add_step() {
        let mut state = AgentState::new("Hello", None);
        state.add_step(AgentStep::new(AgentPhase::Thinking));
        assert_eq!(state.steps.len(), 1);
    }

    #[test]
    fn test_agent_state_complete() {
        let mut state = AgentState::new("Hello", None);
        state.complete("Done!");

        assert_eq!(state.phase, AgentPhase::Complete);
        assert!(state.finished);
        assert_eq!(state.final_response, Some("Done!".to_string()));
        assert!(state.finished_at.is_some());
    }

    #[test]
    fn test_agent_state_fail() {
        let mut state = AgentState::new("Hello", None);
        state.fail("Something went wrong");

        assert_eq!(state.phase, AgentPhase::Error);
        assert!(state.finished);
        assert_eq!(state.error, Some("Something went wrong".to_string()));
        assert!(state.finished_at.is_some());
    }

    #[test]
    fn test_agent_state_progress_percent() {
        let mut state = AgentState::new("Hello", None);

        assert_eq!(state.progress_percent(50), 0);

        state.iteration = 25;
        assert_eq!(state.progress_percent(50), 50);

        state.finished = true;
        assert_eq!(state.progress_percent(50), 100);
    }

    #[test]
    fn test_agent_state_serialization() {
        let state = AgentState::new("Test prompt", Some("System".to_string()));
        let json = serde_json::to_string(&state).unwrap();

        assert!(json.contains("thinking"));
        assert!(json.contains("Test prompt"));

        let deserialized: AgentState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, state.id);
        assert_eq!(deserialized.messages.len(), 2);
    }

    #[test]
    fn test_agent_state_empty_system_prompt() {
        let state = AgentState::new("Hello", Some("".to_string()));
        // Empty string is still a Some, so we have 2 messages
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].content, Some("".to_string()));
    }

    #[test]
    fn test_agent_state_multiple_messages() {
        let mut state = AgentState::new("First message", None);
        state.add_message(Message::assistant("Response 1"));
        state.add_message(Message::user("Second message"));
        state.add_message(Message::assistant("Response 2"));

        assert_eq!(state.messages.len(), 4);
    }

    #[test]
    fn test_agent_state_multiple_steps() {
        let mut state = AgentState::new("Hello", None);

        state.add_step(AgentStep::new(AgentPhase::Thinking));
        state.add_step(AgentStep::new(AgentPhase::Acting));
        state.add_step(AgentStep::new(AgentPhase::Observing));
        state.add_step(AgentStep::new(AgentPhase::Responding));

        assert_eq!(state.steps.len(), 4);
    }

    #[test]
    fn test_agent_state_id_uniqueness() {
        let state1 = AgentState::new("Hello", None);
        let state2 = AgentState::new("Hello", None);

        assert_ne!(state1.id, state2.id);
    }

    #[test]
    fn test_agent_state_progress_edge_cases() {
        let mut state = AgentState::new("Hello", None);

        state.iteration = 50;
        assert_eq!(state.progress_percent(50), 99);

        state.iteration = 100;
        assert_eq!(state.progress_percent(50), 99);
    }

    /// With no iterations to spend there is no progress to report, and the
    /// division that would measure it is `0 / 0`.
    #[test]
    fn an_unfinished_run_with_no_iteration_budget_reports_no_progress() {
        let state = AgentState::new("Hello", None);
        assert_eq!(state.progress_percent(0), 0);
    }

    #[test]
    fn test_agent_state_complete_then_fail_overwrite() {
        let mut state = AgentState::new("Hello", None);

        state.complete("Initial response");
        assert_eq!(state.phase, AgentPhase::Complete);
        assert!(state.finished);
        assert_eq!(state.final_response, Some("Initial response".to_string()));

        state.fail("Error occurred");
        assert_eq!(state.phase, AgentPhase::Error);
        assert!(state.finished);
        assert_eq!(state.error, Some("Error occurred".to_string()));
    }

    #[test]
    fn test_agent_state_fail_then_complete_overwrite() {
        let mut state = AgentState::new("Hello", None);

        state.fail("Error");
        assert_eq!(state.phase, AgentPhase::Error);

        state.complete("Recovery");
        assert_eq!(state.phase, AgentPhase::Complete);
        assert_eq!(state.final_response, Some("Recovery".to_string()));
    }

    #[test]
    fn test_agent_state_tokens_tracking() {
        let mut state = AgentState::new("Hello", None);
        assert_eq!(state.tokens_used, 0);

        state.tokens_used = 500;
        assert_eq!(state.tokens_used, 500);

        state.tokens_used += 300;
        assert_eq!(state.tokens_used, 800);
    }

    #[test]
    fn test_agent_state_iteration_tracking() {
        let mut state = AgentState::new("Hello", None);
        assert_eq!(state.iteration, 0);

        state.iteration = 10;
        assert_eq!(state.iteration, 10);

        state.iteration += 1;
        assert_eq!(state.iteration, 11);
    }

    #[test]
    fn test_agent_state_timing() {
        let state = AgentState::new("Hello", None);
        let start_time = state.started_at;

        assert!(state.finished_at.is_none());

        let mut mutable_state = state;
        std::thread::sleep(std::time::Duration::from_millis(5));
        mutable_state.complete("Done");

        assert!(mutable_state.finished_at.is_some());
        assert!(mutable_state.finished_at.unwrap() > start_time);
    }

    #[test]
    fn test_agent_state_phase_transitions() {
        let mut state = AgentState::new("Hello", None);
        assert_eq!(state.phase, AgentPhase::Thinking);

        state.phase = AgentPhase::Acting;
        assert_eq!(state.phase, AgentPhase::Acting);

        state.phase = AgentPhase::Observing;
        assert_eq!(state.phase, AgentPhase::Observing);

        state.phase = AgentPhase::Responding;
        assert_eq!(state.phase, AgentPhase::Responding);
    }

    #[test]
    fn test_agent_state_with_long_message() {
        let long_message = "a".repeat(10000);
        let state = AgentState::new(long_message.clone(), None);

        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, Some(long_message));
    }

    #[test]
    fn test_agent_state_serialization_with_all_fields() {
        let mut state = AgentState::new("Test", Some("System prompt".to_string()));
        state.iteration = 5;
        state.tokens_used = 1000;

        let tool_call = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let mut step = AgentStep::new(AgentPhase::Acting);
        step.tool_calls = Some(vec![ToolCallResult {
            call: tool_call,
            result: "success".to_string(),
            success: true,
            duration_ms: 100,
        }]);
        state.add_step(step);

        state.add_message(Message::assistant("Response"));
        state.complete("Final answer");

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: AgentState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, state.id);
        assert_eq!(deserialized.iteration, 5);
        assert_eq!(deserialized.tokens_used, 1000);
        assert_eq!(deserialized.steps.len(), 1);
        assert_eq!(deserialized.messages.len(), 3);
        assert!(deserialized.finished);
        assert_eq!(
            deserialized.final_response,
            Some("Final answer".to_string())
        );
    }
}
