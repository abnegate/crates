use crate::client::SourceImage;
use crate::people::PoseImage;
use crate::recipe::Recipe;
use std::collections::HashMap;

/// What one generation pass reads from disk before it is submitted: the
/// recipe it runs with its weights, and the pose and face images its person
/// extras upload.
pub(super) struct Prepared {
    pub(super) recipe: Recipe,
    pub(super) weights: HashMap<String, String>,
    pub(super) pose: Option<PoseImage>,
    pub(super) face: Option<SourceImage>,
}
