use serde::Serialize;

/// A base model a LoRA can be trained on, as
/// [`available_bases`](crate::lora::available_bases) lists it.
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
}
