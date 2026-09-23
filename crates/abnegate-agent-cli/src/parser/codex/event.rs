use serde::Deserialize;

use crate::parser::codex::item::Item;
use crate::parser::codex::reason::Reason;
use crate::parser::codex::token_counts::TokenCounts;

/// One line of `codex exec --json`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "thread.started")]
    Thread {
        #[serde(default)]
        thread_id: Option<String>,
    },
    #[serde(rename = "item.completed")]
    Completed { item: Item },
    #[serde(rename = "turn.completed")]
    Turn {
        #[serde(default)]
        usage: Option<TokenCounts>,
    },
    #[serde(rename = "turn.failed")]
    Failed {
        #[serde(default)]
        error: Option<Reason>,
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(other)]
    Ignored,
}
