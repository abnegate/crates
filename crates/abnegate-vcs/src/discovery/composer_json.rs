use crate::discovery::Requirements;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ComposerJson {
    pub(super) name: Option<String>,
    pub(super) require: Requirements,
    #[serde(rename = "require-dev")]
    pub(super) require_dev: Requirements,
}
