/// The identity a video prompt names: the trigger to keep in the prompt, and
/// the video LoRA trained on it when exactly one is ready.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VideoIdentity {
    /// The trigger word the prompt names, trimmed.
    pub trigger: String,
    /// The ready video LoRA trained on [`trigger`](Self::trigger), when there
    /// is exactly one; otherwise the trigger alone carries the identity.
    pub lora: Option<String>,
}
