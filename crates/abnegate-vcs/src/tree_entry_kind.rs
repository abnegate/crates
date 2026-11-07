use serde::Deserialize;
use std::fmt;

/// What a git tree entry points at, as GitHub and GitLab both name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TreeEntryKind {
    /// A file's content, or the target a symbolic link names.
    Blob,
    /// A directory.
    Tree,
    /// A submodule, pinned at a commit of another repository.
    Commit,
    /// A kind the forge gave that this crate does not know yet.
    #[serde(other)]
    Unknown,
}

impl TreeEntryKind {
    /// The kind as GitHub and GitLab spell it, or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Tree => "tree",
            Self::Commit => "commit",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for TreeEntryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tree_entry_kind_reads_and_prints_as_the_forges_spell_it() {
        for (kind, spelled) in [
            (TreeEntryKind::Blob, "blob"),
            (TreeEntryKind::Tree, "tree"),
            (TreeEntryKind::Commit, "commit"),
            (TreeEntryKind::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::from_value::<TreeEntryKind>(json!(spelled)).unwrap(),
                kind
            );
            assert_eq!(kind.as_str(), spelled);
            assert_eq!(kind.to_string(), spelled);
        }
    }

    #[test]
    fn a_kind_no_forge_has_named_yet_is_unknown() {
        assert_eq!(
            serde_json::from_value::<TreeEntryKind>(json!("tag")).unwrap(),
            TreeEntryKind::Unknown
        );
    }
}
