use crate::discovery::Requirements;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct PackageJson {
    pub(super) name: Option<String>,
    pub(super) dependencies: Requirements,
    #[serde(rename = "devDependencies")]
    pub(super) dev_dependencies: Requirements,
}
