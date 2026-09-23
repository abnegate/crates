//! Job management and state tracking.
//!
//! This module provides types for tracking the lifecycle of command
//! execution jobs, including state transitions and concurrent access.

mod registry;
mod state;

pub use registry::{JobEntry, JobRegistry};
pub use state::JobState;
