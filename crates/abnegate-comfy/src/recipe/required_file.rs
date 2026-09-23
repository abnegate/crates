use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RequiredFile {
    pub filename: String,
    pub directory: String,
}
