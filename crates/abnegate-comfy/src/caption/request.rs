use crate::caption::CaptionImage;
use serde::Deserialize;

/// A set of images to caption for one LoRA.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct CaptionRequest {
    /// The word the LoRA will learn its subject by, which leads every
    /// written caption and is kept out of what follows it.
    #[serde(default)]
    pub trigger: Option<String>,
    /// The images, in order.
    pub images: Vec<CaptionImage>,
}

impl CaptionRequest {
    /// Captions `images`, with no trigger word.
    pub fn new(images: Vec<CaptionImage>) -> Self {
        Self {
            trigger: None,
            images,
        }
    }

    /// Sets the word the LoRA will learn its subject by.
    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }
}
