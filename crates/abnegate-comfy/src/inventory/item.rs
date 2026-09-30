use serde::Serialize;

/// One weight [`scan`](crate::inventory::scan) found, joined to the recipe that
/// runs it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[non_exhaustive]
pub struct InventoryItem {
    /// The weight file's name.
    pub filename: String,
    /// The [`Recipe::id`](crate::recipe::Recipe::id) that runs it.
    pub recipe_id: String,
    /// `checkpoint`, `diffusion_model` or `lora`.
    pub kind: String,
    /// The name shown to a person choosing a model.
    pub label: String,
    /// The folder under the models directory it was found in.
    pub directory: String,
    /// Its size in bytes, or 0 when it could not be read.
    pub size: u64,
    /// When it last changed, as RFC 3339, when that could be read.
    pub modified_at: Option<String>,
    /// Whether every file its recipe needs is installed.
    pub ready: bool,
    /// The files still missing when it is not [`ready`](Self::ready);
    /// otherwise every file its recipe needs, a LoRA first.
    pub required_files: Vec<String>,
    /// Whether it is a LoRA loaded over a base.
    pub adapter: bool,
    /// How a prompt for it is written: `clip_scene` or `edit_instruction`.
    pub prompt_mode: String,
}
