use serde::Deserialize;

use crate::parser::lenient;

/// The `state` of a command claude has just begun.
const STARTED: &str = "started";

/// A `command_lifecycle` line: a command claude runs on its own account
/// starting or ending, a wakeup firing among them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Lifecycle {
    #[serde(deserialize_with = "lenient")]
    state: Option<String>,
    /// What woke claude, on a command a wakeup started.
    #[serde(deserialize_with = "lenient")]
    wakeup_source: Option<String>,
}

impl Lifecycle {
    /// Whether this line is a wakeup firing.
    pub(crate) fn woke(&self) -> bool {
        self.state.as_deref() == Some(STARTED) && self.wakeup_source.is_some()
    }
}
