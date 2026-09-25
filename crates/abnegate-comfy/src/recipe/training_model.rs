/// The exact model components a supported training graph loads.
///
/// This is resolved only from the catalog's explicit `training` metadata. A
/// recipe id, prompt mode, or process-wide checkpoint is never enough to select
/// a trainer because those are presentation and inference concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrainingModel {
    /// A FLUX graph, which loads everything from one checkpoint.
    #[non_exhaustive]
    Flux {
        /// The checkpoint's filename.
        checkpoint: String,
    },
    /// A Qwen image-edit graph, which loads its three components separately.
    #[non_exhaustive]
    QwenEdit {
        /// The diffusion model's filename.
        unet: String,
        /// The text encoder's filename.
        clip: String,
        /// The image autoencoder's filename.
        vae: String,
    },
}
