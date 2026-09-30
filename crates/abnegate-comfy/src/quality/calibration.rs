use serde::Serialize;

/// Whether callers may compare the score with the FLUX health thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QualityCalibration {
    /// A FLUX adapter, whose score the FLUX health thresholds apply to.
    FluxHealthBands,
    /// A model no thresholds have been measured for: compare scores only
    /// with each other.
    Uncalibrated,
}
