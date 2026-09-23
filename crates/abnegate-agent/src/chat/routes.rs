use serde::Deserialize;

use super::route::Route;

/// A page of LiteLLM `/v2/model/info`.
#[derive(Deserialize)]
pub(super) struct Routes {
    pub(super) data: Vec<Route>,
    #[serde(default)]
    pub(super) total_pages: Option<u64>,
}
