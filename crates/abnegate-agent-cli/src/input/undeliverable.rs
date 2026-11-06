//! Why an image on the message being answered cannot reach the agent.

use thiserror::Error;

use crate::image::Rejection;
use crate::kind::AgentKind;

/// Why an image on the message being answered cannot reach the agent, which
/// fails the turn before the agent starts.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Undeliverable {
    /// The agent cannot be shown the image.
    #[error("the image attached to this message cannot be given to {agent}: {rejection}")]
    #[non_exhaustive]
    Refused {
        /// The agent the image was for.
        agent: AgentKind,
        /// Why it cannot be shown it.
        rejection: Rejection,
    },
    /// The file the agent was to open the image from could not be written.
    #[error("the image attached to this message could not be saved for codex: {0}")]
    Unsaved(#[from] std::io::Error),
}

impl Undeliverable {
    /// `agent` cannot be shown the image, for `rejection`.
    pub fn refused(agent: AgentKind, rejection: Rejection) -> Self {
        Self::Refused { agent, rejection }
    }
}
