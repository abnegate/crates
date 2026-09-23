use async_trait::async_trait;
use uuid::Uuid;

use super::{Session, SessionError, SessionSummary};

/// Trait for session storage backends
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), SessionError>;

    /// Load a session by ID
    async fn load(&self, id: Uuid) -> Result<Session, SessionError>;

    /// Delete a session
    async fn delete(&self, id: Uuid) -> Result<(), SessionError>;

    /// List all sessions
    async fn list(&self) -> Result<Vec<SessionSummary>, SessionError>;

    /// Get the most recent session
    async fn most_recent(&self) -> Result<Option<Session>, SessionError>;
}
