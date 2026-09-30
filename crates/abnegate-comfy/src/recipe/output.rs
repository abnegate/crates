use serde::Deserialize;

/// How a recipe's graph hands back what it made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RecipeOutput {
    /// A `PreviewImage` node, which keeps the result in ComfyUI's temporary
    /// storage rather than its output folder.
    PreviewImage,
}
