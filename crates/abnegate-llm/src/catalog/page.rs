use crate::catalog::entry::ModelEntry;
use serde::Deserialize;
use serde::Serialize;

/// Default number of models returned by one browse.
pub const DEFAULT_PAGE_SIZE: usize = 20;
/// Largest page a caller may ask a provider for.
pub const MAXIMUM_PAGE_SIZE: usize = 100;

/// One page of browse results and the cursor that continues it.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ModelPage {
    /// The models on this page, in the order asked for.
    pub models: Vec<ModelEntry>,
    /// The cursor that fetches the next page, or `None` on the last.
    #[serde(default)]
    pub next_cursor: Option<String>,
}
