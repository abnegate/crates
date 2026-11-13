//! Error types for confined and unconfined command execution.

mod daemon;
mod executor;
mod group_registry;
mod job;
mod marker;
mod niceness;
mod protocol;

pub use daemon::DaemonError;
pub use executor::ExecutorError;
pub use group_registry::GroupRegistryError;
pub use job::JobError;
pub use marker::MarkerError;
pub use niceness::NicenessError;
pub use protocol::ProtocolError;
