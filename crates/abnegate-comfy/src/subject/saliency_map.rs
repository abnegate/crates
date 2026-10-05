use abnegate_vision::gravity::Rectangle;

/// A copy of the map the model located a subject on, kept to render masks
/// of that image's crops without running the model again.
pub(crate) struct SaliencyMap {
    pub(crate) values: Vec<f32>,
    pub(crate) size: (u32, u32),
    pub(crate) content: Rectangle,
}
