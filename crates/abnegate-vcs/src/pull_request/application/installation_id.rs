use serde::Deserialize;
use serde::Serialize;

/// One installation of a GitHub App on an account or organisation, which an
/// [`crate::pull_request::InstallationToken`] is scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstallationId(u64);

impl InstallationId {
    /// The installation GitHub numbered `value`.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The number GitHub gave the installation.
    pub const fn get(self) -> u64 {
        self.0
    }
}
