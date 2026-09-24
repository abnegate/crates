#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrainingAdapter {
    pub recipe_id: String,
    pub huggingface_base: String,
}
