/// What a working tree currently differs from its last commit by.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    /// Every path with a change, tracked or not.
    pub files_changed: Vec<String>,
    /// Lines added, against the last commit.
    pub insertions: u32,
    /// Lines removed, against the last commit.
    pub deletions: u32,
    /// The diff against the last commit, cut at 50 kB. A cut diff ends in a
    /// line saying so.
    pub diff_text: String,
}
