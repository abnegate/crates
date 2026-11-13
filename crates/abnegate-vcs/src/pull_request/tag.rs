use crate::commit_sha::CommitSha;

/// A tag of a repository, and the commit it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tag {
    /// The tag's name, without `refs/tags/`, as GitHub spells it.
    pub name: String,
    /// The commit it points at, through its tag object when it is annotated.
    pub sha: CommitSha,
}

impl Tag {
    /// The tag `name`, pointing at `sha`.
    pub fn new(name: impl Into<String>, sha: CommitSha) -> Self {
        Self {
            name: name.into(),
            sha,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_holds_what_it_was_made_with() {
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();

        let tag = Tag::new("pkg/v1.2.3", sha.clone());

        assert_eq!(tag.name, "pkg/v1.2.3");
        assert_eq!(tag.sha, sha);
    }
}
