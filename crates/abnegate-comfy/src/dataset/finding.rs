use crate::dataset::Concern;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Finding {
    pub concern: Concern,
    pub detail: String,
}
