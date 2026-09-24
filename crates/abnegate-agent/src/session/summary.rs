use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use super::Session;

/// Session metadata for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(alias = "project_dir")]
    pub project_directory: Option<String>,
    pub finished: bool,
}

impl From<&Session> for SessionSummary {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id,
            title: session.title.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            project_directory: session.project_directory.clone(),
            finished: session.state.finished,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::AgentState;

    #[test]
    fn test_session_summary_from_session() {
        let state = AgentState::new("Test prompt", None);
        let session = Session::new(state, "My Session", Some("/project".to_string()));

        let summary = SessionSummary::from(&session);

        assert_eq!(summary.id, session.id);
        assert_eq!(summary.title, "My Session");
        assert_eq!(summary.project_directory, Some("/project".to_string()));
        assert!(!summary.finished);
    }

    #[test]
    fn test_session_summary_finished() {
        let mut state = AgentState::new("Test prompt", None);
        state.complete("All done");

        let session = Session::new(state, "Finished Session", None);
        let summary = SessionSummary::from(&session);

        assert!(summary.finished);
    }

    #[test]
    fn test_session_summary_serialization() {
        let state = AgentState::new("Test prompt", None);
        let session = Session::new(state, "Test Session", Some("/project".to_string()));
        let summary = SessionSummary::from(&session);

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("Test Session"));
        assert!(json.contains("/project"));

        let deserialized: SessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, summary.id);
    }

    #[test]
    fn test_session_summary_without_project() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "No Project", None);
        let summary = SessionSummary::from(&session);

        assert!(summary.project_directory.is_none());
    }

    #[test]
    fn test_session_summary_preserves_timestamps() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Timestamp", None);
        let summary = SessionSummary::from(&session);

        assert_eq!(summary.created_at, session.created_at);
        assert_eq!(summary.updated_at, session.updated_at);
    }

    #[test]
    fn test_session_summary_clone() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Clone", None);
        let summary = SessionSummary::from(&session);
        let cloned = summary.clone();

        assert_eq!(cloned.id, summary.id);
        assert_eq!(cloned.title, summary.title);
        assert_eq!(cloned.finished, summary.finished);
    }

    #[test]
    fn test_session_summary_debug() {
        let state = AgentState::new("Test", None);
        let session = Session::new(state, "Debug", None);
        let summary = SessionSummary::from(&session);
        let debug_str = format!("{:?}", summary);

        assert!(debug_str.contains("SessionSummary"));
        assert!(debug_str.contains("Debug"));
    }

    #[test]
    fn test_session_summary_serialization_roundtrip() {
        let mut state = AgentState::new("Test", None);
        state.complete("Done");

        let session = Session::new(state, "Roundtrip", Some("/path".to_string()));
        let summary = SessionSummary::from(&session);

        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: SessionSummary = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, summary.id);
        assert_eq!(deserialized.title, summary.title);
        assert_eq!(deserialized.project_directory, summary.project_directory);
        assert_eq!(deserialized.finished, summary.finished);
    }
}
