use crate::lora::TrainMethod;
use crate::lora::TrainSubject;
use std::path::PathBuf;

/// What one base accepts for one training method: the subjects it trains,
/// the recipe that runs the weight and the directory it is published in, and
/// the files that have to be installed before a run starts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TrainRule {
    /// [`Recipe::id`](crate::recipe::Recipe::id) of the base.
    pub base: String,
    /// The method the rule covers.
    pub method: TrainMethod,
    /// The subjects the base trains with this method.
    pub subjects: Vec<TrainSubject>,
    /// The recipe written into the published weight's sidecar, which runs it.
    pub recipe_id: String,
    /// The directory under the models root the weight is published in: the
    /// one the recipe's loader reads, so `loras` for a
    /// [`TrainMethod::Lora`], and for a [`TrainMethod::Finetune`]
    /// `checkpoints` when the recipe loads a checkpoint or `diffusion_models`
    /// when it loads a UNet. Training on a rule that names any other directory
    /// fails as a configuration error.
    pub directory: String,
    /// Files, relative to the models root, that have to be installed before a
    /// run starts, such as `checkpoints/base.safetensors`.
    pub required: Vec<PathBuf>,
}

impl TrainRule {
    /// Lets `base` train `method` on [`TrainSubject::Other`], publishing the
    /// weight in `directory` for the recipe `recipe_id`, with nothing required
    /// beyond the base.
    pub fn new(
        base: impl Into<String>,
        method: TrainMethod,
        recipe_id: impl Into<String>,
        directory: impl Into<String>,
    ) -> Self {
        Self {
            base: base.into(),
            method,
            subjects: vec![TrainSubject::Other],
            recipe_id: recipe_id.into(),
            directory: directory.into(),
            required: Vec::new(),
        }
    }

    /// Replaces the subjects the rule trains.
    pub fn with_subjects(mut self, subjects: impl IntoIterator<Item = TrainSubject>) -> Self {
        self.subjects = subjects.into_iter().collect();
        self
    }

    /// Adds a file, relative to the models root, that has to be installed
    /// before a run starts.
    pub fn with_required(mut self, file: impl Into<PathBuf>) -> Self {
        self.required.push(file.into());
        self
    }
}
