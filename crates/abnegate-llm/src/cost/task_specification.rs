use crate::cost::TaskCategory;

/// A quantity of work of one category, under a label the estimate reports it by.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TaskSpecification {
    pub label: String,
    pub category: TaskCategory,
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
