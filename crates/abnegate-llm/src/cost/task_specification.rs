use crate::cost::TaskCategory;

/// A quantity of work of one category, under a label the estimate reports it by.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TaskSpecification {
    /// The name the estimate lists this task under, in its breakdown or among
    /// the tasks it left unassigned.
    pub label: String,
    /// The kind of work, which decides the models that can do it.
    pub category: TaskCategory,
    /// How much of the work there is, in the unit the chosen model's price is
    /// quoted in: tokens, images, characters, or seconds of audio or video.
    pub quantity: u32,
}

impl TaskSpecification {
    /// `quantity` units of `category` work, reported as `label`.
    pub fn new(label: impl Into<String>, category: TaskCategory, quantity: u32) -> Self {
        Self {
            label: label.into(),
            category,
            quantity,
        }
    }
}
