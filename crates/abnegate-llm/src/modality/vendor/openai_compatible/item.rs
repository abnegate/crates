use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct Item {
    pub(super) index: usize,
    pub(super) embedding: Vec<f32>,
}
