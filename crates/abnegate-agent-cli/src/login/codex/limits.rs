use std::time::Duration;

use crate::login::output::COMMAND;

/// How long codex has to print its prompt, by default.
const PROMPT: Duration = Duration::from_secs(20);

/// How long codex may run past its code's expiry, by default.
const GRACE: Duration = Duration::from_secs(30);

/// How long each stage of a device sign-in may take. A limit too long for
/// this host's clock to reach, such as [`Duration::MAX`], waits about 30
/// years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How long codex has to print its link, code and expiry: 20 seconds by
    /// default.
    pub prompt: Duration,
    /// How long codex may keep running past the expiry it printed before it
    /// is stopped: 30 seconds by default.
    pub grace: Duration,
    /// How long a short-lived command such as `codex logout` may run before
    /// it is stopped: 30 seconds by default.
    pub command: Duration,
}

impl Limits {
    /// These limits, with `prompt` for codex to print its prompt in.
    pub fn with_prompt(mut self, prompt: Duration) -> Self {
        self.prompt = prompt;
        self
    }

    /// These limits, letting codex run `grace` past its code's expiry.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// These limits, letting a short-lived command such as `codex logout`
    /// run for `command`.
    pub fn with_command(mut self, command: Duration) -> Self {
        self.command = command;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            prompt: PROMPT,
            grace: GRACE,
            command: COMMAND,
        }
    }
}
