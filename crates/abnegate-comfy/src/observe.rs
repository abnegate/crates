//! Optional hook so a host can record request outcomes without this crate
//! depending on its metrics stack.

use std::sync::OnceLock;
use std::time::Duration;

/// Called once per ComfyUI request with the workflow kind, the outcome, and how
/// long it took.
pub type RequestObserver = fn(kind: &'static str, status: &'static str, duration: Duration);

static OBSERVER: OnceLock<RequestObserver> = OnceLock::new();

/// Install the hook. Later calls are ignored, so the first one wins.
pub fn observe_requests(observer: RequestObserver) {
    let _ = OBSERVER.set(observer);
}

pub(crate) fn record(kind: &'static str, status: &'static str, duration: Duration) {
    if let Some(observer) = OBSERVER.get() {
        observer(kind, status, duration);
    }
}
