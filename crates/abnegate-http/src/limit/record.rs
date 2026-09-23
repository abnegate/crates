use std::time::Instant;

/// How many requests one key has made in its current window.
pub(super) struct Record {
    pub(super) window_start: Instant,
    pub(super) count: u32,
}

impl Record {
    pub(super) fn new(window_start: Instant) -> Self {
        Self {
            window_start,
            count: 0,
        }
    }
}
