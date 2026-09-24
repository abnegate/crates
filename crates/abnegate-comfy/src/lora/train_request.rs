use crate::lora::TrainImage;
use serde::Deserialize;

/// A LoRA to train: its name, the base recipe it is trained on, and its images.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct TrainRequest {
    /// Filename the adapter is published under, with or without `.safetensors`.
    pub name: String,
    /// Recipe id of the base the adapter is trained on.
    pub base: String,
    /// Word an identity adapter is prompted with. Edit bases need none.
    #[serde(default)]
    pub trigger: Option<String>,
    /// The training set.
    pub images: Vec<TrainImage>,
}

impl TrainRequest {
    /// Trains the adapter `name` on the base recipe `base` from `images`, with
    /// no trigger word.
    pub fn new(name: impl Into<String>, base: impl Into<String>, images: Vec<TrainImage>) -> Self {
        Self {
            name: name.into(),
            base: base.into(),
            trigger: None,
            images,
        }
    }

    /// Sets the word an identity adapter is prompted with.
    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }
}
