use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

use super::{Session, SessionError, SessionStore, SessionSummary};

const SESSIONS_DIRECTORY: &str = "sessions";
const SESSION_EXTENSION: &str = "json";

/// Sessions stored as one pretty-printed JSON file each, in one directory.
pub struct FileSessionStore {
    directory: PathBuf,
}

impl FileSessionStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// A store in `~/.{application}/sessions`, or nothing on a host with no
    /// home directory.
    pub fn default_location(application: &str) -> Option<Self> {
        dirs::home_dir().map(|home| {
            Self::new(
                home.join(format!(".{application}"))
                    .join(SESSIONS_DIRECTORY),
            )
        })
    }

    async fn ensure_directory(&self) -> Result<(), SessionError> {
        fs::create_dir_all(&self.directory).await?;
        Ok(())
    }

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.directory.join(format!("{id}.{SESSION_EXTENSION}"))
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionError> {
        self.ensure_directory().await?;

        let path = self.session_path(session.id);
        let json = serde_json::to_string_pretty(session)?;
        fs::write(&path, json).await?;

        Ok(())
    }

    async fn load(&self, id: Uuid) -> Result<Session, SessionError> {
        let path = self.session_path(id);

        if !path.exists() {
            return Err(SessionError::NotFound(id));
        }

        let json = fs::read_to_string(&path).await?;
        let session: Session = serde_json::from_str(&json)?;

        Ok(session)
    }

    async fn delete(&self, id: Uuid) -> Result<(), SessionError> {
        let path = self.session_path(id);

        if path.exists() {
            fs::remove_file(&path).await?;
        }

        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        self.ensure_directory().await?;

        let mut entries = fs::read_dir(&self.directory).await?;
        let mut summaries = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.extension().and_then(|extension| extension.to_str()) != Some(SESSION_EXTENSION)
            {
                continue;
            }

            if let Ok(json) = fs::read_to_string(&path).await
                && let Ok(session) = serde_json::from_str::<Session>(&json)
            {
                summaries.push(SessionSummary::from(&session));
            }
        }

        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at));

        Ok(summaries)
    }

    async fn most_recent(&self) -> Result<Option<Session>, SessionError> {
        let summaries = self.list().await?;

        if let Some(summary) = summaries.first() {
            let session = self.load(summary.id).await?;
            Ok(Some(session))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentState;
    use tempfile::TempDir;

    fn create_test_session(prompt: &str, title: &str) -> Session {
        let state = AgentState::new(prompt, None);
        Session::new(state, title, None)
    }

    #[tokio::test]
    async fn test_file_session_store_new() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());
        assert_eq!(store.directory, directory.path());
    }

    #[test]
    fn test_file_session_store_default_location() {
        let store = FileSessionStore::default_location("agent");
        if let Some(home) = dirs::home_dir() {
            let store = store.expect("a home directory holds a default location");
            assert_eq!(store.directory, home.join(".agent").join("sessions"));
        }
    }

    #[tokio::test]
    async fn test_save_and_load_session() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let session = create_test_session("Test prompt", "Test Session");
        let session_id = session.id;

        store.save(&session).await.unwrap();

        let path = store.session_path(session_id);
        assert!(path.exists());

        let loaded = store.load(session_id).await.unwrap();
        assert_eq!(loaded.id, session_id);
        assert_eq!(loaded.title, "Test Session");
    }

    #[tokio::test]
    async fn test_load_nonexistent_session() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let result = store.load(Uuid::new_v4()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SessionError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_delete_session() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let session = create_test_session("Test prompt", "Test Session");
        let session_id = session.id;

        store.save(&session).await.unwrap();
        assert!(store.session_path(session_id).exists());

        store.delete(session_id).await.unwrap();

        assert!(!store.session_path(session_id).exists());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_session() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let result = store.delete(Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let sessions = store.list().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        // Saving does not restamp a session, so the order under test comes from
        // the timestamps themselves rather than from when each one is written.
        let session1 = create_test_session("Prompt 1", "Session 1");
        let mut session2 = create_test_session("Prompt 2", "Session 2");
        let mut session3 = create_test_session("Prompt 3", "Session 3");
        let oldest = session1.updated_at;
        session2.updated_at = oldest + chrono::Duration::seconds(1);
        session3.updated_at = oldest + chrono::Duration::seconds(2);
        store.save(&session1).await.unwrap();
        store.save(&session2).await.unwrap();
        store.save(&session2).await.unwrap();
        store.save(&session3).await.unwrap();

        let sessions = store.list().await.unwrap();
        assert_eq!(sessions.len(), 3);

        assert_eq!(sessions[0].title, "Session 3");
        assert_eq!(sessions[1].title, "Session 2");
        assert_eq!(sessions[2].title, "Session 1");
    }

    #[tokio::test]
    async fn test_list_ignores_non_json_files() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());
        store.ensure_directory().await.unwrap();

        let session = create_test_session("Test", "Valid Session");
        store.save(&session).await.unwrap();

        fs::write(directory.path().join("not-json.txt"), "This is not JSON")
            .await
            .unwrap();

        fs::write(directory.path().join("invalid.json"), "{ invalid json }")
            .await
            .unwrap();

        let sessions = store.list().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Valid Session");
    }

    #[tokio::test]
    async fn test_most_recent_empty() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let result = store.most_recent().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_most_recent_returns_latest() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let session1 = create_test_session("Prompt 1", "Old Session");
        store.save(&session1).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let session2 = create_test_session("Prompt 2", "New Session");
        store.save(&session2).await.unwrap();

        let most_recent = store.most_recent().await.unwrap().unwrap();
        assert_eq!(most_recent.title, "New Session");
    }

    #[tokio::test]
    async fn test_session_path_format() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());
        let id = Uuid::new_v4();

        let path = store.session_path(id);
        assert!(path.to_string_lossy().ends_with(&format!("{}.json", id)));
    }

    #[tokio::test]
    async fn test_ensure_dir_creates_directory() {
        let directory = TempDir::new().unwrap();
        let nested_path = directory.path().join("nested").join("sessions");
        let store = FileSessionStore::new(nested_path.clone());

        assert!(!nested_path.exists());
        store.ensure_directory().await.unwrap();
        assert!(nested_path.exists());
    }

    #[tokio::test]
    async fn test_save_updates_existing_session() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let mut session = create_test_session("Initial prompt", "Initial Title");
        let session_id = session.id;

        store.save(&session).await.unwrap();

        session.title = "Updated Title".to_string();
        store.save(&session).await.unwrap();

        let loaded = store.load(session_id).await.unwrap();
        assert_eq!(loaded.title, "Updated Title");
    }

    #[tokio::test]
    async fn test_session_with_project_dir() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let state = AgentState::new("Test", None);
        let session = Session::new(
            state,
            "Project Session",
            Some("/path/to/project".to_string()),
        );

        store.save(&session).await.unwrap();
        let loaded = store.load(session.id).await.unwrap();

        assert_eq!(
            loaded.project_directory,
            Some("/path/to/project".to_string())
        );
    }

    #[tokio::test]
    async fn test_session_preserves_state() {
        let directory = TempDir::new().unwrap();
        let store = FileSessionStore::new(directory.path().to_path_buf());

        let mut state = AgentState::new("Test prompt", Some("System prompt".to_string()));
        state.complete("Final response");

        let session = Session::new(state, "Finished Session", None);
        store.save(&session).await.unwrap();

        let loaded = store.load(session.id).await.unwrap();
        assert!(loaded.state.finished);
        assert!(loaded.state.final_response.is_some());
    }

    #[tokio::test]
    async fn test_concurrent_saves() {
        let directory = TempDir::new().unwrap();
        let store = std::sync::Arc::new(FileSessionStore::new(directory.path().to_path_buf()));

        let mut handles = vec![];

        for i in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                let session =
                    create_test_session(&format!("Prompt {}", i), &format!("Session {}", i));
                store_clone.save(&session).await.unwrap();
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let sessions = store.list().await.unwrap();
        assert_eq!(sessions.len(), 10);
    }
}
