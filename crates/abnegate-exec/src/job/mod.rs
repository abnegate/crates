//! Job management and state tracking.
//!
//! This module provides types for tracking the lifecycle of command
//! execution jobs, including state transitions and concurrent access.

mod entry;
mod registry;
mod state;

pub use entry::JobEntry;
pub use registry::JobRegistry;
pub use state::JobState;
