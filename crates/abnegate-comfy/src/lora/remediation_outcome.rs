use serde::Serialize;

/// The result of trying the configured image upscaler before rejecting a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RemediationOutcome {
    /// The upscaled image passed screening and is trained on.
    Used,
    /// The upscaled image was screened out again.
    StillRejected,
    /// The upscale itself failed, so the original was screened out.
    Failed,
}
