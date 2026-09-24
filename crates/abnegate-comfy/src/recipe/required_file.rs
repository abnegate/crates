use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct RequiredFile {
    pub filename: String,
    pub directory: String,
}
