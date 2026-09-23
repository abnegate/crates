use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct TrainBase {
    pub id: String,
    pub label: String,
    pub edit: bool,
}
