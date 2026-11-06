use std::collections::BTreeMap;

use abnegate_secret::sanitize;
use serde::Deserialize;
use serde_json::Value;

use crate::activity::subject;
use crate::parser::codex::collab_agent::CollabAgent;
use crate::parser::codex::collab_status::CollabStatus;
use crate::parser::codex::collab_tool::CollabTool;
use crate::parser::lenient;
use crate::subagent::Subagent;
use crate::subagent::SubagentStatus;

/// A call codex made to one of its collaboration tools, which start, talk
/// to, wait on and close the agents it runs beside itself. Every field is
/// read leniently: news of a collaborator is never worth losing the line
/// over.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Collab {
    #[serde(deserialize_with = "lenient")]
    tool: Option<CollabTool>,
    #[serde(deserialize_with = "lenient")]
    receiver_thread_ids: Option<Vec<String>>,
    #[serde(deserialize_with = "lenient")]
    prompt: Option<String>,
    /// Each agent's last known state by its thread id, kept raw so that one
    /// agent in an unknown shape does not hide the others.
    #[serde(deserialize_with = "lenient")]
    agents_states: Option<BTreeMap<String, Value>>,
    #[serde(deserialize_with = "lenient")]
    status: Option<CollabStatus>,
}

impl Collab {
    /// What this call said about each agent it named, one update per agent.
    pub(crate) fn subagents(self) -> Vec<Subagent> {
        let Some(tool) = self.tool else {
            return Vec::new();
        };
        let spawned = tool == CollabTool::SpawnAgent;
        let closed = tool == CollabTool::CloseAgent && self.status == Some(CollabStatus::Completed);
        let mut subagents: Vec<Subagent> = Vec::new();
        if spawned {
            let description = self.prompt.as_deref().and_then(described);
            let receivers = self.receiver_thread_ids.unwrap_or_default();
            for id in receivers.into_iter().filter(|id| !id.is_empty()) {
                subagents.push(Subagent {
                    description: description.clone(),
                    status: Some(SubagentStatus::Running),
                    ..Subagent::new(id)
                });
            }
        }
        for (id, state) in self.agents_states.unwrap_or_default() {
            let Ok(CollabAgent {
                status: Some(status),
                message,
            }) = serde_json::from_value(state)
            else {
                continue;
            };
            let Some(status) = status.subagent(closed) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let index = match subagents.iter().position(|subagent| subagent.id == id) {
                Some(index) => index,
                None => {
                    subagents.push(Subagent::new(id));
                    subagents.len() - 1
                }
            };
            let subagent = &mut subagents[index];
            subagent.status = Some(status);
            match status {
                SubagentStatus::Completed => {
                    subagent.summary = message.as_deref().and_then(worded);
                }
                SubagentStatus::Failed => subagent.error = message.as_deref().and_then(worded),
                SubagentStatus::Running | SubagentStatus::Stopped => {}
            }
        }
        if spawned && self.status == Some(CollabStatus::Failed) {
            for subagent in &mut subagents {
                subagent.status = Some(SubagentStatus::Failed);
            }
        }
        subagents
    }
}

/// What an agent was asked to do, on one line: the prompt's first line, cut
/// and cleaned as a tool call's subject is.
fn described(prompt: &str) -> Option<String> {
    subject(&serde_json::json!({ "prompt": prompt }).to_string())
}

/// An agent's own report, cleaned of anything a reader must not see.
fn worded(message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    Some(sanitize(message).into_owned())
}
