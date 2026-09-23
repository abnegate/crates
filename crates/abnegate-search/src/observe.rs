//! Optional hook so a host can record search outcomes without this crate
//! depending on its metrics stack.

use std::sync::OnceLock;
use std::time::Duration;

/// Called once per search with the outcome, how long it took, and how many
/// results came back.
pub type SearchObserver = fn(status: &'static str, duration: Duration, results: usize);

static OBSERVER: OnceLock<SearchObserver> = OnceLock::new();

/// Install the hook. Later calls are ignored, so the first one wins.
pub fn observe_searches(observer: SearchObserver) {
    let _ = OBSERVER.set(observer);
}

pub(crate) fn record(status: &'static str, duration: Duration, results: usize) {
    if let Some(observer) = OBSERVER.get() {
        observer(status, duration, results);
    }
}
