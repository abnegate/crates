use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Tells a [`Walk`](super::Walk) nobody is waiting for it any more, by
/// raising the flag it watches once the guard is dropped.
///
/// A walk runs on a blocking thread, which cannot be cancelled, so a caller
/// that gives up drops this guard instead and the walk ends at its next
/// entry.
pub(super) struct Abandon(pub(super) Arc<AtomicBool>);

impl Drop for Abandon {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
