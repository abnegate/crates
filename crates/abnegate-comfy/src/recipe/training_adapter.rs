/// How a LoRA trained on a base recipe is run once it is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrainingAdapter {
    /// The adapter recipe that loads the LoRA.
    pub recipe_id: String,
    /// The Hugging Face base model the LoRA is recorded as derived from.
    pub huggingface_base: String,
}
