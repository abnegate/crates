use serde::Deserialize;

/// How a recipe's graph hands back what it made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RecipeOutput {
    /// A `PreviewImage` node, which keeps the result in ComfyUI's temporary
    /// storage rather than its output folder.
    PreviewImage,
    /// A `SaveWEBM` node, which encodes a video clip.
    SaveWebm,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_pinned() {
        for (wire, output) in [
            ("preview_image", RecipeOutput::PreviewImage),
            ("save_webm", RecipeOutput::SaveWebm),
        ] {
            assert_eq!(
                serde_json::from_value::<RecipeOutput>(serde_json::json!(wire)).unwrap(),
                output
            );
        }
    }
}
