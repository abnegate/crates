use abnegate_llm::Message;

use super::ContextUsage;
use super::Summary;

/// What [`prepare`](super::prepare) hands back: the messages to send, their
/// usage, and the checkpoint they were projected through.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Prepared {
    /// The messages to send, with covered history replaced by the checkpoint.
    pub messages: Vec<Message>,
    /// What they are estimated to cost, and whether compaction ran or failed.
    pub usage: ContextUsage,
    /// The checkpoint they were projected through, which is a new revision
    /// when this preparation compacted.
    pub summary: Option<Summary>,
}
