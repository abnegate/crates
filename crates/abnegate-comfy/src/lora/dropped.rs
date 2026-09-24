use crate::screening::Rejection;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Dropped {
    pub filename: String,
    pub reason: Rejection,
}
