/// The exact model components a supported training graph loads.
///
/// This is resolved only from the catalog's explicit `training` metadata. A
/// recipe id, prompt mode, or process-wide checkpoint is never enough to select
/// a trainer because those are presentation and inference concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrainingModel {
    Flux {
        checkpoint: String,
    },
    QwenEdit {
        unet: String,
        clip: String,
        vae: String,
    },
}
