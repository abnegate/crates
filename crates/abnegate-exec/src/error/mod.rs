//! Error types for confined and unconfined command execution.

mod daemon;
mod executor;
mod job;
mod protocol;

pub use daemon::DaemonError;
pub use executor::ExecutorError;
pub use job::JobError;
pub use protocol::ProtocolError;
