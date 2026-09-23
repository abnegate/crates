//! Command execution engine.
//!
//! This module provides the core functionality for spawning and managing
//! command execution with streaming output, timeouts, and cancellation.

mod command;
mod config;
mod confinement;
mod ending;
mod environment_policy;
mod handshake;
mod job_handle;
mod output_kind;
mod output_limiter;
mod output_stream;
mod process_group;
mod stdin_handle;
mod supervisor;

pub use command::CommandExecutor;
pub use config::DEFAULT_BUFFER_SIZE;
pub use config::DEFAULT_MAX_OUTPUT_BYTES;
pub use config::DEFAULT_TIMEOUT_MS;
pub use config::ExecutorConfig;
pub use config::GRACE_PERIOD;
pub use confinement::Backend;
pub use confinement::Confinement;
pub use confinement::ConfinementError;
pub use confinement::ConfinementMode;
pub use confinement::HOST_BACKEND;
pub use confinement::Invocation;
pub use environment_policy::DEFAULT_ENVIRONMENT_ALLOWLIST;
pub use environment_policy::EnvironmentPolicy;
pub use job_handle::JobHandle;
pub use output_kind::OutputKind;
pub use output_limiter::OutputLimiter;
pub use process_group::ProcessGroup;
pub use stdin_handle::StdinHandle;
