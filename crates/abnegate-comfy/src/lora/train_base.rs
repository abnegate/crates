use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[non_exhaustive]
pub struct TrainBase {
    pub id: String,
    pub label: String,
    pub edit: bool,
}
