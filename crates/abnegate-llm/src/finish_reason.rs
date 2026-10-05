//! The OpenAI-compatible words a completion gives for why it ended.

/// A turn that ended normally.
///
/// Claude reports `success` and codex `completed`. A loop that takes a reply
/// as final only on `stop` would never accept a turn carrying an agent's own
/// word and would run to its iteration limit instead, so a bridge from an
/// agent reports this.
pub const STOP: &str = "stop";

/// A reply cut off by its token limit.
pub const LENGTH: &str = "length";
