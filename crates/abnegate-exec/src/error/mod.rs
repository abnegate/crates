//! Error types for confined and unconfined command execution.

mod daemon;
mod executor;
mod job;
mod marker;
mod protocol;

pub use daemon::DaemonError;
pub use executor::ExecutorError;
pub use job::JobError;
pub use marker::MarkerError;
pub use protocol::ProtocolError;
