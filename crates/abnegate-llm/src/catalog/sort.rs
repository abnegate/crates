use serde::Deserialize;
use serde::Serialize;

/// How to order browse results.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelSort {
    #[default]
    Relevance,
    DownloadsDesc,
    DownloadsAsc,
    NameAsc,
    NameDesc,
    SizeAsc,
    SizeDesc,
    ParamsAsc,
    ParamsDesc,
    UpdatedDesc,
    UpdatedAsc,
}
