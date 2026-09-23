/// One `<<<<<<< / ======= / >>>>>>>` block, split into the two sides it offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictHunk {
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}
