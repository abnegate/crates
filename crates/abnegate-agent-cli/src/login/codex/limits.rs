use std::time::Duration;

/// How long codex has to print its prompt, by default.
const PROMPT: Duration = Duration::from_secs(20);

/// How long codex may run past its code's expiry, by default.
const GRACE: Duration = Duration::from_secs(30);

/// How long each stage of a device sign-in may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How long codex has to print its link, code and expiry: 20 seconds by
    /// default.
    pub prompt: Duration,
    /// How long codex may keep running past the expiry it printed before it
    /// is stopped: 30 seconds by default.
    pub grace: Duration,
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
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            prompt: PROMPT,
            grace: GRACE,
        }
    }
}
