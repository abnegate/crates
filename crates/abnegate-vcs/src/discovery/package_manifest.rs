use crate::discovery::requirements::Requirements;
use serde::Deserialize;

/// The parts of a `package.json` this reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct PackageManifest {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) dependencies: Requirements,
    #[serde(default, rename = "devDependencies")]
    pub(super) dev_dependencies: Requirements,
}
