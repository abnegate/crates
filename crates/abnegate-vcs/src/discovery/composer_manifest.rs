use crate::discovery::requirements::Requirements;
use serde::Deserialize;

/// The parts of a `composer.json` this reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct ComposerManifest {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) require: Requirements,
    #[serde(default, rename = "require-dev")]
    pub(super) require_dev: Requirements,
}
