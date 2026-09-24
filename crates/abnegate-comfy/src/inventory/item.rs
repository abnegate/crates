use serde::Serialize;

/// One weight [`scan`](crate::inventory::scan) found, joined to the recipe that
/// runs it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[non_exhaustive]
pub struct InventoryItem {
    pub filename: String,
    pub recipe_id: String,
    pub kind: String,
    pub label: String,
    pub directory: String,
    pub size: u64,
    pub modified_at: Option<String>,
    pub ready: bool,
    pub required_files: Vec<String>,
    pub adapter: bool,
    pub prompt_mode: String,
}
