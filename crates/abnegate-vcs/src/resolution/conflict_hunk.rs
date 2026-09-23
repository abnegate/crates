/// One `<<<<<<< / ======= / >>>>>>>` block, split into the two sides it offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictHunk {
    /// The lines our side offers.
    pub ours: Vec<String>,
    /// The lines their side offers.
    pub theirs: Vec<String>,
}
