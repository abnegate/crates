use crate::object_id::ObjectId;
use crate::pull_request::TreeEntryKind;

/// The modes git gives a blob that is a regular file: one not executable, and
/// one executable. A symbolic link is a blob too, with mode `120000`.
const REGULAR_FILE_MODES: &[&str] = &["100644", "100755"];

/// One entry of a git tree: a file, a symbolic link, a directory or a
/// submodule.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TreeEntry {
    /// Its path: its name within the tree, or, in a tree read recursively, its
    /// path from the tree's root.
    pub path: String,
    /// Its file mode as git writes it, such as `100644`, `100755`, `120000`
    /// for a symbolic link, `040000` for a directory or `160000` for a
    /// submodule.
    pub mode: String,
    /// What it points at.
    pub kind: TreeEntryKind,
    /// The object it points at: a blob, a tree, or a submodule's commit.
    pub sha: ObjectId,
    /// The size of a blob in bytes, or `None` for an entry that is not one.
    pub size: Option<u64>,
}

impl TreeEntry {
    /// The entry at `path`, with file mode `mode`, pointing at the `kind` of
    /// object `sha` names. It has no size until [`Self::with_size`] gives one.
    pub fn new(
        path: impl Into<String>,
        mode: impl Into<String>,
        kind: TreeEntryKind,
        sha: ObjectId,
    ) -> Self {
        Self {
            path: path.into(),
            mode: mode.into(),
            kind,
            sha,
            size: None,
        }
    }

    /// The size of the blob it points at, in bytes.
    #[must_use]
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Whether it is a regular file: a blob with mode `100644` or `100755`,
    /// and so neither a symbolic link, a directory nor a submodule.
    pub fn regular_file(&self) -> bool {
        self.kind == TreeEntryKind::Blob && REGULAR_FILE_MODES.contains(&self.mode.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mode: &str, kind: TreeEntryKind) -> TreeEntry {
        TreeEntry::new(
            "entry",
            mode,
            kind,
            ObjectId::parse(&"a".repeat(40)).unwrap(),
        )
    }

    #[test]
    fn only_a_blob_with_a_regular_file_mode_is_a_regular_file() {
        for (mode, kind, regular) in [
            ("100644", TreeEntryKind::Blob, true),
            ("100755", TreeEntryKind::Blob, true),
            ("120000", TreeEntryKind::Blob, false),
            ("040000", TreeEntryKind::Tree, false),
            ("160000", TreeEntryKind::Commit, false),
            ("100644", TreeEntryKind::Unknown, false),
            ("100644", TreeEntryKind::Tree, false),
        ] {
            assert_eq!(
                entry(mode, kind).regular_file(),
                regular,
                "{kind} with mode {mode}"
            );
        }
    }

    #[test]
    fn an_entry_has_a_size_only_once_given_one() {
        let entry = entry("100644", TreeEntryKind::Blob);
        assert_eq!(entry.size, None);
        assert_eq!(entry.with_size(12).size, Some(12));
    }
}
