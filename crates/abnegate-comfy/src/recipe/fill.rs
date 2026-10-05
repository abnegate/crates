use std::collections::HashMap;

/// What [`Recipe::apply`](crate::recipe::Recipe::apply) writes into a recipe's
/// slots.
#[non_exhaustive]
pub struct Fill<'a> {
    /// Written into the prompt slot.
    pub prompt: &'a str,
    /// Written into the seed slot.
    pub seed: u64,
    /// The weight filename for each weight slot, by slot name.
    pub weights: HashMap<&'a str, &'a str>,
    /// The uploaded source image. `Some` selects the recipe's source-image graph.
    pub source: Option<&'a str>,
    /// The uploaded pose image for the recipe's pose slot. `None` removes the
    /// ControlNet branch that slot feeds.
    pub pose: Option<&'a str>,
}

impl<'a> Fill<'a> {
    /// Fills the recipe's bare graph, with no source image.
    pub fn new(prompt: &'a str, seed: u64, weights: HashMap<&'a str, &'a str>) -> Self {
        Self {
            prompt,
            seed,
            weights,
            source: None,
            pose: None,
        }
    }

    /// Fills the recipe's source-image graph with `source`, an image already
    /// uploaded to ComfyUI.
    pub fn with_source(mut self, source: &'a str) -> Self {
        self.source = Some(source);
        self
    }

    /// Steers the recipe's ControlNet branch with `pose`, an image already
    /// uploaded to ComfyUI. That branch loads the recipe's
    /// [`pose_required_files`](crate::recipe::Recipe::pose_required_files),
    /// which the host makes sure are installed first.
    pub fn with_pose(mut self, pose: &'a str) -> Self {
        self.pose = Some(pose);
        self
    }
}
