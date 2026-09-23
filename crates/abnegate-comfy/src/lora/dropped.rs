use crate::screening::Rejection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Dropped {
    pub filename: String,
    pub reason: Rejection,
}
