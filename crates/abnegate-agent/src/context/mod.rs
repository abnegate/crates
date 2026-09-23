//! Canonical, token-aware conversation projections.
//!
//! History is append-only. When a request would overflow its model's context,
//! [`prepare`] folds consumed history into a [`Summary`] kept beside it, and
//! [`project`] replays the history through that summary. Nothing here ever
//! edits an entry: a checkpoint that no longer matches the fingerprint of what
//! it covers is refused rather than trusted.
//!
//! [`estimate`] and [`tokens`] price a request before it is sent;
//! [`trim_history`] and [`build_chat_prompt`] serve models that take one flat
//! prompt rather than a list of messages.

mod breakdown;
mod compact;
mod coverage;
mod entry;
mod error;
mod estimate;
mod policy;
mod prepared;
mod prompt;
mod source;
mod status;
mod summary;
mod usage;

pub use breakdown::ContextBreakdown;
pub use compact::coverage;
pub use compact::prepare;
pub use compact::project;
pub use compact::validate;
pub use coverage::Coverage;
pub use entry::Entry;
pub use error::ContextError;
pub use estimate::estimate;
pub use estimate::tokens;
pub use policy::Policy;
pub use prepared::Prepared;
pub use prompt::build_chat_prompt;
pub use prompt::trim_history;
pub use source::ContextSource;
pub use status::ContextStatus;
pub use summary::Summary;
pub use usage::ContextUsage;
