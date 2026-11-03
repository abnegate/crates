use std::collections::HashMap;

/// The recipe one generation pass runs, by its catalog id, and the weights it
/// fills into it.
pub(super) struct Plan {
    pub(super) recipe: String,
    pub(super) weights: HashMap<String, String>,
}
