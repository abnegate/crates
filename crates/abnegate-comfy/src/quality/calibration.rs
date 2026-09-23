use serde::Serialize;

/// Whether callers may compare the score with the FLUX health thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QualityCalibration {
    FluxHealthBands,
    Uncalibrated,
}
