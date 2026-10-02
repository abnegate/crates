//! Counts from one [`index_tree`](crate::index_tree) or [`index_texts`](crate::index_texts) pass.

/// Counts from one [`index_tree`](crate::index_tree) or
/// [`index_texts`](crate::index_texts) pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Tree the pass indexed, using `/`.
    pub root: String,
    /// Files whose chunks were written.
    pub files: usize,
    /// Windows written across those files.
    pub chunks: usize,
    /// Windows that received an embedding.
    pub embedded: usize,
    /// Files skipped because their content hash already matched.
    pub skipped: usize,
}

impl Stats {
    /// An empty pass over `root`.
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_only_the_root() {
        let stats = Stats::new("/src");
        assert_eq!(stats.root, "/src");
        assert_eq!(stats.files, 0);
        assert_eq!(stats.chunks, 0);
    }
}
