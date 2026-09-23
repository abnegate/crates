use serde::Serialize;

/// The result of trying the configured image upscaler before rejecting a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RemediationOutcome {
    Used,
    StillRejected,
    Failed,
}
