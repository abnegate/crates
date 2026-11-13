use serde::Deserialize;

/// The kind of git object a reference points at, as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ObjectKind {
    /// A commit, as a branch or a lightweight tag points at.
    Commit,
    /// An annotated tag object, which in turn names what it tags.
    Tag,
    /// A tree.
    Tree,
    /// A blob.
    Blob,
    /// GitHub gave no kind, or one this crate does not know yet.
    #[default]
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_kind_is_read_from_the_word_github_sends() {
        for (word, kind) in [
            ("commit", ObjectKind::Commit),
            ("tag", ObjectKind::Tag),
            ("tree", ObjectKind::Tree),
            ("blob", ObjectKind::Blob),
            ("submodule", ObjectKind::Unknown),
        ] {
            assert_eq!(
                serde_json::from_value::<ObjectKind>(json!(word)).unwrap(),
                kind,
                "{word}"
            );
        }
        assert_eq!(ObjectKind::default(), ObjectKind::Unknown);
    }
}
