//! Command execution engine.
//!
//! This module provides the core functionality for spawning and managing
//! command execution with streaming output, timeouts, and cancellation.

mod command;
mod confinement;
mod limits;
mod process_group;

pub use command::{CommandExecutor, JobHandle, OutputKind, StdinHandle};
pub use confinement::{
    Backend, Confinement, ConfinementError, ConfinementMode, HOST_BACKEND, Invocation,
};
pub use limits::{
    DEFAULT_BUFFER_SIZE, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_TIMEOUT_MS, ExecutorConfig,
    GRACE_PERIOD, OutputLimiter,
};
pub use process_group::ProcessGroup;
