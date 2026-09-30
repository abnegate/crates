use serde::Deserialize;

/// How a prompt for a recipe is best written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PromptMode {
    /// A description of the scene to render. The default.
    #[default]
    ClipScene,
    /// An instruction saying what to change in a source image.
    EditInstruction,
}
