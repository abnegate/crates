use crate::lora::TrainMethod;
use crate::lora::TrainSubject;
use serde::Serialize;

/// A base model a weight can be trained on, as
/// [`trainable_bases`](crate::lora::trainable_bases) lists it.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[non_exhaustive]
pub struct TrainBase {
    /// The [`Recipe::id`](crate::recipe::Recipe::id) to name as a training base.
    pub id: String,
    /// The name shown to a person choosing a base.
    pub label: String,
    /// Whether it trains an image-editing model, whose images come in
    /// target and reference pairs with an instruction rather than a
    /// trigger word.
    pub edit: bool,
    /// The subjects a request on this base may name, in the order its rules
    /// give them.
    pub subjects: Vec<TrainSubject>,
    /// The methods a request on this base may name, each one something the
    /// configuration can run.
    pub methods: Vec<TrainMethod>,
}
