use abnegate_llm::Message;

use super::{ContextUsage, Summary};

/// What [`prepare`](super::prepare) hands back: the messages to send, their
/// usage, and the checkpoint they were projected through.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub messages: Vec<Message>,
    pub usage: ContextUsage,
    pub summary: Option<Summary>,
}
