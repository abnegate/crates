//! Why a session could not be carried or moved.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::kind::AgentKind;

/// Why [`Session::carry`](crate::Session::carry) or
/// [`Session::relocate`](crate::Session::relocate) left a session where it
/// was.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The home holds no file for the session, or holds it only through a
    /// link.
    #[error("the {agent} session {id} has no file in its home")]
    #[non_exhaustive]
    Missing {
        /// The agent whose session it is.
        agent: AgentKind,
        /// The session's id.
        id: String,
    },
    /// The id is not one an agent names a session with: it is empty or holds
    /// a character other than an ASCII letter, digit or `-`, so it could name
    /// a path outside the session folders.
    #[error("{id:?} is not a session id")]
    #[non_exhaustive]
    Invalid {
        /// The id as it was given.
        id: String,
    },
    /// Writing the session at `path` failed, or a directory on the way to it
    /// is a link or not a directory.
    #[error("could not write the session to {}: {source}", path.display())]
    #[non_exhaustive]
    Filesystem {
        /// The file, or the directory, that could not be written.
        path: PathBuf,
        /// Why writing it failed.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use super::SessionError;
    use crate::kind::AgentKind;

    #[test]
    fn each_failure_names_what_it_was_about() {
        let missing = SessionError::Missing {
            agent: AgentKind::Codex,
            id: "019b2c41".to_string(),
        };
        let invalid = SessionError::Invalid {
            id: "../secret".to_string(),
        };
        let filesystem = SessionError::Filesystem {
            path: PathBuf::from("/home/agent/projects"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        };

        assert_eq!(
            missing.to_string(),
            "the codex session 019b2c41 has no file in its home"
        );
        assert_eq!(invalid.to_string(), "\"../secret\" is not a session id");
        assert_eq!(
            filesystem.to_string(),
            "could not write the session to /home/agent/projects: denied"
        );
    }
}
