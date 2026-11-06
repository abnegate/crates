//! Why the images on the message being answered cannot reach the agent.

use thiserror::Error;
use tokio::task::JoinError;

use crate::image::ImageError;
use crate::kind::AgentKind;

/// Why the images on the message being answered cannot reach the agent,
/// which fails the turn before the agent starts.
#[derive(Debug, Error)]
pub(crate) enum InputError {
    /// The agent cannot be shown the image, for `reason`.
    #[error("the image attached to this message cannot be given to {agent}: {reason}")]
    Refused {
        agent: AgentKind,
        reason: ImageError,
    },
    /// The file the agent was to open the image from could not be written.
    #[error("the image attached to this message could not be saved for codex: {0}")]
    Unsaved(#[from] std::io::Error),
    /// The runtime shut down before the images were prepared.
    #[error("the images attached to this message could not be prepared: {0}")]
    Interrupted(JoinError),
}

impl InputError {
    pub(crate) fn refused(agent: AgentKind, reason: ImageError) -> Self {
        Self::Refused { agent, reason }
    }
}
