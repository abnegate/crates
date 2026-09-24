use crate::cost::TaskCategory;

/// A quantity of work of one category, under a label the estimate reports it by.
#[derive(Debug, Clone)]
pub struct TaskSpecification {
    pub label: String,
    pub category: TaskCategory,
    pub quantity: u32,
}
