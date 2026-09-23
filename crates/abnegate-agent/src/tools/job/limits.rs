use std::time::Duration;

use super::{LOG_CHECK_INTERVAL, MAX_JOB_LIFETIME, MAX_JOB_LOG_BYTES};

/// What a job is held to.
///
/// A value rather than the constants read directly, so a test can prove the
/// reaper without writing [`MAX_JOB_LOG_BYTES`] to disk or waiting out
/// [`MAX_JOB_LIFETIME`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    pub(super) lifetime: Duration,
    pub(super) log_bytes: u64,
    pub(super) log_check: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            lifetime: MAX_JOB_LIFETIME,
            log_bytes: MAX_JOB_LOG_BYTES,
            log_check: LOG_CHECK_INTERVAL,
        }
    }
}
