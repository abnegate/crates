use serde::Deserialize;

use crate::parser::codex::collab_agent_status::CollabAgentStatus;
use crate::parser::lenient;

/// One agent's last known state, as a collaboration call reports it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CollabAgent {
    #[serde(deserialize_with = "lenient")]
    pub(crate) status: Option<CollabAgentStatus>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) message: Option<String>,
}
