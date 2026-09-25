use crate::client::Error;
use crate::recipe::sanitize_weight_filename;

/// The exact model components a supported training graph loads.
///
/// [`Recipe::training_model`](crate::recipe::Recipe::training_model) resolves
/// one from the catalog's explicit `training` metadata; a caller that chooses
/// the model to train instead names its files with [`TrainingModel::flux`] or
/// [`TrainingModel::qwen_edit`]. A recipe id, prompt mode, or process-wide
/// checkpoint is never enough to select a trainer because those are
/// presentation and inference concerns.
///
/// ```
/// use abnegate_comfy::recipe::TrainingModel;
///
/// let model = TrainingModel::flux("flux1-dev-fp8.safetensors")?;
/// assert!(matches!(model, TrainingModel::Flux { .. }));
/// assert!(TrainingModel::flux("../flux1-dev-fp8.safetensors").is_err());
/// # Ok::<(), abnegate_comfy::Error>(())
/// ```
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

impl TrainingModel {
    /// A FLUX graph that loads everything from the checkpoint named
    /// `checkpoint`.
    ///
    /// Fails with [`Error::Configuration`] when `checkpoint` is empty, longer
    /// than 256 bytes, or holds a `/`, a `\` or `..`.
    pub fn flux(checkpoint: &str) -> Result<Self, Error> {
        Ok(Self::Flux {
            checkpoint: sanitize_weight_filename(checkpoint)?,
        })
    }

    /// A Qwen image-edit graph that loads its diffusion model from `unet`,
    /// its text encoder from `clip` and its image autoencoder from `vae`.
    ///
    /// Fails with [`Error::Configuration`] when any of them is empty, longer
    /// than 256 bytes, or holds a `/`, a `\` or `..`.
    pub fn qwen_edit(unet: &str, clip: &str, vae: &str) -> Result<Self, Error> {
        Ok(Self::QwenEdit {
            unet: sanitize_weight_filename(unet)?,
            clip: sanitize_weight_filename(clip)?,
            vae: sanitize_weight_filename(vae)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEIGHT: &str = "weight.safetensors";

    #[test]
    fn each_constructor_fills_its_variant() {
        assert_eq!(
            TrainingModel::flux("flux1-dev-fp8.safetensors").expect("a bare filename"),
            TrainingModel::Flux {
                checkpoint: "flux1-dev-fp8.safetensors".to_string(),
            }
        );
        assert_eq!(
            TrainingModel::qwen_edit(
                "qwen_image_edit_2511_fp8mixed.safetensors",
                "qwen_2.5_vl_7b_fp8_scaled.safetensors",
                "qwen_image_vae.safetensors",
            )
            .expect("bare filenames"),
            TrainingModel::QwenEdit {
                unet: "qwen_image_edit_2511_fp8mixed.safetensors".to_string(),
                clip: "qwen_2.5_vl_7b_fp8_scaled.safetensors".to_string(),
                vae: "qwen_image_vae.safetensors".to_string(),
            }
        );
    }

    #[test]
    fn a_filename_that_is_not_bare_is_refused() {
        let oversized = "x".repeat(257);
        for name in [
            "",
            "../outside.safetensors",
            "checkpoints/flux.safetensors",
            "checkpoints\\flux.safetensors",
            oversized.as_str(),
        ] {
            assert!(TrainingModel::flux(name).is_err(), "{name:?}");
            assert!(
                TrainingModel::qwen_edit(name, WEIGHT, WEIGHT).is_err(),
                "unet {name:?}"
            );
            assert!(
                TrainingModel::qwen_edit(WEIGHT, name, WEIGHT).is_err(),
                "clip {name:?}"
            );
            assert!(
                TrainingModel::qwen_edit(WEIGHT, WEIGHT, name).is_err(),
                "vae {name:?}"
            );
        }
    }
}
