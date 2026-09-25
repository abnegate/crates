use abnegate_config::ApplicationError;
use thiserror::Error;

use crate::chat::ChatError;
use crate::context::ContextError;
#[cfg(feature = "mcp")]
use crate::mcp::McpConfigError;
#[cfg(feature = "mcp")]
use crate::mcp::McpError;
use crate::run::RunError;
use crate::session::SessionError;
use crate::tool::ToolError;

/// Any failure this crate reports, for a caller that composes several of its
/// modules and wants one error to return.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// A tool could not run.
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    /// A run stopped short of an answer: its provider or its context
    /// failed, or it ran out of iterations or of usable answers.
    #[error("Run error: {0}")]
    Run(#[from] RunError),
    /// A name could not be an application's.
    #[error("Application error: {0}")]
    Application(#[from] ApplicationError),
    /// A conversation could not be prepared for its model.
    #[error("Context error: {0}")]
    Context(#[from] ContextError),
    /// A saved session could not be found, read or written.
    #[error("Session error: {0}")]
    Session(#[from] SessionError),
    /// A conversation store refused or failed an operation.
    #[error("Chat store error: {0}")]
    Chat(#[from] ChatError),
    /// An MCP server could not be started, reached or called.
    #[cfg(feature = "mcp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
    #[error("MCP error: {0}")]
    Mcp(#[from] McpError),
    /// An MCP configuration could not be read.
    #[cfg(feature = "mcp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
    #[error("MCP config error: {0}")]
    McpConfig(#[from] McpConfigError),
    /// A value could not be written as JSON, or read back from it.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// A file or a process could not be read, written or started.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A result whose failure is any of this crate's errors.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn test_tool_error_display() {
        let error: Error = ToolError::NotFound("read_file".to_string()).into();
        assert_eq!(error.to_string(), "Tool error: Tool not found: read_file");
    }

    #[test]
    fn test_iteration_limit_error() {
        let error: Error = RunError::IterationLimit.into();
        assert_eq!(error.to_string(), "Run error: Maximum iterations exceeded");
    }

    #[test]
    fn test_empty_error() {
        let error: Error = RunError::Empty.into();
        assert_eq!(
            error.to_string(),
            "Run error: The model answered with nothing usable too many times in a row"
        );
    }

    #[test]
    fn test_context_error_display() {
        let error: Error = ContextError::Integrity("fingerprint changed".to_string()).into();
        assert_eq!(
            error.to_string(),
            "Context error: Conversation checkpoint integrity error: fingerprint changed"
        );
    }

    #[test]
    fn test_session_error_display() {
        let id = Uuid::new_v4();
        let error: Error = SessionError::NotFound(id).into();
        assert_eq!(
            error.to_string(),
            format!("Session error: Session not found: {id}")
        );
    }

    #[test]
    fn test_chat_error_display() {
        let error: Error = ChatError::Busy.into();
        assert_eq!(
            error.to_string(),
            "Chat store error: This chat already has an active response"
        );
    }

    #[test]
    fn test_serialization_error_from_serde() {
        let json_error: serde_json::Error = serde_json::from_str::<i32>("invalid").unwrap_err();
        let error: Error = json_error.into();
        assert!(matches!(error, Error::Serialization(_)));
        assert!(error.to_string().contains("Serialization error"));
    }

    #[test]
    fn test_io_error_from_std() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        let error: Error = io_error.into();
        assert!(matches!(error, Error::Io(_)));
        assert!(error.to_string().contains("IO error"));
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn test_mcp_error_display() {
        let error: Error = McpError::Call("server went away".to_string()).into();
        assert_eq!(
            error.to_string(),
            "MCP error: MCP tool call failed: server went away"
        );
    }

    #[test]
    fn test_result_type_ok() {
        fn answer() -> Result<i32> {
            Ok(42)
        }
        assert_eq!(answer().unwrap(), 42);
    }

    #[test]
    fn test_result_type_err() {
        let result: Result<i32> = Err(RunError::Empty.into());
        assert!(result.is_err());
    }

    #[test]
    fn test_error_debug() {
        let error: Error = ToolError::Execution("test".to_string()).into();
        let debug = format!("{error:?}");
        assert!(debug.contains("Tool"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_error_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}

        assert_send::<Error>();
        assert_sync::<Error>();
    }
}
