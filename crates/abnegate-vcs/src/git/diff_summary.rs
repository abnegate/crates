/// What a working tree currently differs from its last commit by.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub files_changed: Vec<String>,
    pub insertions: u32,
    pub deletions: u32,
    pub diff_text: String,
}
