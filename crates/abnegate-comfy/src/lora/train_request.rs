use crate::lora::TrainImage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TrainRequest {
    pub name: String,
    pub base: String,
    #[serde(default)]
    pub trigger: Option<String>,
    pub images: Vec<TrainImage>,
}
