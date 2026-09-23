//! Saved agent runs: a [`Session`] wraps an [`AgentState`] with a title and
//! timestamps, and a [`SessionStore`] keeps them. [`FileSessionStore`] keeps
//! each one as a JSON file.

mod error;
mod file;
mod store;
mod summary;

pub use error::SessionError;
pub use file::FileSessionStore;
pub use store::SessionStore;
pub use summary::SessionSummary;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentState;

/// A saved run: its [`AgentState`] under a title, with the directory it
/// worked in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// The run's own id, so a session and its state never disagree.
    pub id: Uuid,
    pub title: String,
    pub state: AgentState,
    pub created_at: DateTime<Utc>,
    /// Moved forward by [`update`](Self::update), never by saving.
    pub updated_at: DateTime<Utc>,
    /// The project the run worked in, when it had one.
    #[serde(alias = "project_dir")]
    pub project_directory: Option<String>,
}

impl Session {
    /// A session for `state`, created and updated now.
    pub fn new(
        state: AgentState,
        title: impl Into<String>,
        project_directory: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: state.id,
            title: title.into(),
            state,
            created_at: now,
            updated_at: now,
            project_directory,
        }
    }

    /// Replace the state and move `updated_at` forward.
    pub fn update(&mut self, state: AgentState) {
        self.state = state;
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_new() {
        let state = AgentState::new("Test prompt", None);
        let session = Session::new(
            state.clone(),
            "Test Session",
            Some("/tmp/project".to_string()),
        );

        assert_eq!(session.id, state.id);
        assert_eq!(session.title, "Test Session");
        assert_eq!(session.project_directory, Some("/tmp/project".to_string()));
        assert!(!session.state.finished);
    }

    #[test]
    fn test_session_update() {
        let state = AgentState::new("Test prompt", None);
        let mut session = Session::new(state.clone(), "Test Session", None);

        let initial_updated = session.updated_at;

        // Wait a tiny bit to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut new_state = AgentState::new("Updated prompt", None);
        new_state.complete("Done!");
        session.update(new_state);

        assert!(session.state.finished);
        assert!(session.updated_at > initial_updated);
    }

    #[test]
    fn test_session_serialization() {
        let state = AgentState::new("Test prompt", None);
        let session = Session::new(state, "Test Session", None);

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("Test Session"));
        assert!(json.contains("Test prompt"));

        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, session.id);
        assert_eq!(deserialized.title, session.title);
    }

    #[test]
    fn test_session_new_without_project_dir() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state.clone(), "No Project", None);

        assert_eq!(session.id, state.id);
        assert_eq!(session.title, "No Project");
        assert!(session.project_directory.is_none());
    }

    #[test]
    fn test_session_timestamps_on_creation() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Timestamp Test", None);

        assert_eq!(session.created_at, session.updated_at);
    }

    #[test]
    fn test_session_update_changes_timestamp() {
        let state = AgentState::new("Test", None);
        let mut session = Session::new(state, "Update Test", None);
        let original_updated = session.updated_at;

        // Wait to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        let new_state = AgentState::new("Updated", None);
        session.update(new_state);

        assert!(session.updated_at > original_updated);
        assert!(session.created_at < session.updated_at);
    }

    #[test]
    fn test_session_update_preserves_id() {
        let state = AgentState::new("Test", None);
        let mut session = Session::new(state.clone(), "ID Test", None);
        let original_id = session.id;

        let new_state = AgentState::new("New", None);
        session.update(new_state.clone());

        assert_eq!(session.id, original_id);
        assert_eq!(session.state.id, new_state.id);
    }

    #[test]
    fn test_session_with_empty_title() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "", None);

        assert_eq!(session.title, "");
    }

    #[test]
    fn test_session_with_long_title() {
        let state = AgentState::new("Test", None);
        let long_title = "a".repeat(1000);
        let session = Session::new(state, long_title.clone(), None);

        assert_eq!(session.title, long_title);
    }

    #[test]
    fn test_session_with_unicode_title() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Session: Testing Unicode", None);

        assert_eq!(session.title, "Session: Testing Unicode");
    }

    #[test]
    fn test_session_clone() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Clone Test", Some("/path".to_string()));
        let cloned = session.clone();

        assert_eq!(cloned.id, session.id);
        assert_eq!(cloned.title, session.title);
        assert_eq!(cloned.project_directory, session.project_directory);
        assert_eq!(cloned.created_at, session.created_at);
    }

    #[test]
    fn test_session_serialization_roundtrip_full() {
        let mut state = AgentState::new("Test prompt", Some("System".to_string()));
        state.complete("Done");

        let session = Session::new(state, "Full Roundtrip", Some("/full/path".to_string()));

        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, session.id);
        assert_eq!(deserialized.title, session.title);
        assert_eq!(deserialized.project_directory, session.project_directory);
        assert!(deserialized.state.finished);
        assert_eq!(deserialized.state.final_response, Some("Done".to_string()));
    }

    #[test]
    fn test_session_debug() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Debug Test", None);
        let debug_str = format!("{:?}", session);

        assert!(debug_str.contains("Session"));
        assert!(debug_str.contains("Debug Test"));
    }

    #[test]
    fn test_session_with_completed_state() {
        let mut state = AgentState::new("Test", None);
        state.complete("Final response");

        let session = Session::new(state, "Completed", None);

        assert!(session.state.finished);
        assert_eq!(
            session.state.final_response,
            Some("Final response".to_string())
        );

        let summary = SessionSummary::from(&session);
        assert!(summary.finished);
    }

    #[test]
    fn test_session_with_failed_state() {
        let mut state = AgentState::new("Test", None);
        state.fail("Error message");

        let session = Session::new(state, "Failed", None);

        assert!(session.state.finished);
        assert_eq!(session.state.error, Some("Error message".to_string()));

        let summary = SessionSummary::from(&session);
        assert!(summary.finished);
    }

    #[test]
    fn test_session_update_to_completed() {
        let state = AgentState::new("Test", None);
        let mut session = Session::new(state, "To Complete", None);

        let summary_before = SessionSummary::from(&session);
        assert!(!summary_before.finished);

        let mut new_state = AgentState::new("Updated", None);
        new_state.complete("Done");
        session.update(new_state);

        let summary_after = SessionSummary::from(&session);
        assert!(summary_after.finished);
    }

    #[test]
    fn test_session_multiple_updates() {
        let state = AgentState::new("Initial", None);
        let mut session = Session::new(state, "Multiple Updates", None);

        for i in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let new_state = AgentState::new(format!("Update {}", i), None);
            session.update(new_state);
        }

        assert!(session.updated_at > session.created_at);
    }

    #[test]
    fn test_session_with_special_path_characters() {
        let state = AgentState::new("Test", None);
        let session = Session::new(
            state,
            "Special Path",
            Some("/path/with spaces/and-dashes/under_scores".to_string()),
        );

        assert_eq!(
            session.project_directory,
            Some("/path/with spaces/and-dashes/under_scores".to_string())
        );

        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.project_directory, session.project_directory);
    }

    #[test]
    fn test_session_id_matches_state_id() {
        let state = AgentState::new("Test", None);
        let state_id = state.id;
        let session = Session::new(state, "ID Match", None);

        assert_eq!(session.id, state_id);
    }
}
