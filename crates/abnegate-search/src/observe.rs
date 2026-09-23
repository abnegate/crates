//! Optional hook so a host can record search outcomes without this crate
//! depending on its metrics stack.

use std::sync::OnceLock;
use std::time::Duration;

use crate::outcome::Outcome;

/// Called once per search with the outcome, how long it took, and how many
/// results came back.
pub type SearchObserver = fn(outcome: Outcome, duration: Duration, results: usize);

static OBSERVER: OnceLock<SearchObserver> = OnceLock::new();

/// Install the hook. Later calls are ignored, so the first one wins.
pub fn observe_searches(observer: SearchObserver) {
    let _ = OBSERVER.set(observer);
}

pub(crate) fn record(outcome: Outcome, duration: Duration, results: usize) {
    if let Some(observer) = OBSERVER.get() {
        observer(outcome, duration, results);
    }
}
