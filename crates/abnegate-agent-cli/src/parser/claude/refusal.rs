//! A window claude refused a turn on.

use abnegate_llm::Limit;
use abnegate_llm::Window;
use chrono::DateTime;
use chrono::Utc;

use crate::event::AgentEvent;

/// A window claude refused the turn on, as one rate-limit report described
/// it: a later report that refuses nothing in words of its own never mixes
/// its window or reset time into this one's.
#[derive(Debug, Clone)]
pub(super) struct Refusal {
    pub(super) message: String,
    pub(super) resets_at: Option<DateTime<Utc>>,
    pub(super) window: Option<Window>,
}

impl Refusal {
    /// The turn refused on this window, in `message`.
    pub(super) fn limited(&self, message: String) -> AgentEvent {
        limited(message, false, self.resets_at, self.window.as_ref())
    }
}

/// The turn refused past a limit, in `message`, with whatever claude said of
/// the window it refused on and when that window resets.
pub(super) fn limited(
    message: String,
    credits: bool,
    resets_at: Option<DateTime<Utc>>,
    window: Option<&Window>,
) -> AgentEvent {
    let mut limit = Limit::new(message).with_credits(credits);
    if let Some(resets_at) = resets_at {
        limit = limit.with_resets_at(resets_at);
    }
    if let Some(window) = window {
        limit = limit.with_window(window.clone());
    }
    AgentEvent::Limited(limit)
}
