/// A ready weight a prompt selects by naming its trigger word: an image LoRA
/// over its installed base, or a fine-tuned checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Identity {
    /// The weight file's name, which [`Config::checkpoint`](crate::Config::checkpoint)
    /// takes to generate with it.
    pub filename: String,
    /// The trigger word it was trained on, trimmed.
    pub trigger: String,
}
