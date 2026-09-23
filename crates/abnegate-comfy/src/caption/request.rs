use crate::caption::CaptionImage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CaptionRequest {
    #[serde(default)]
    pub trigger: Option<String>,
    pub images: Vec<CaptionImage>,
}
