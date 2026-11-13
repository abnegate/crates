use crate::commit_sha::CommitSha;
use crate::pull_request::Tagger;

/// An annotated tag to create: its name, its message, the commit it points
/// at, and who signs it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TagRequest {
    /// The tag's name, without `refs/tags/`.
    pub tag: String,
    /// The message the tag carries.
    pub message: String,
    /// The commit the tag points at.
    pub commit: CommitSha,
    /// Who signs the tag.
    pub tagger: Tagger,
}

impl TagRequest {
    /// The tag `tag` on `commit`, carrying `message`, signed by `tagger`.
    pub fn new(
        tag: impl Into<String>,
        message: impl Into<String>,
        commit: CommitSha,
        tagger: Tagger,
    ) -> Self {
        Self {
            tag: tag.into(),
            message: message.into(),
            commit,
            tagger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_request_holds_what_it_was_made_with() {
        let commit = CommitSha::parse(&"a".repeat(40)).unwrap();
        let tagger = Tagger::new("Release Bot", "release@example.com");

        let request = TagRequest::new("v1.2.0", "Version 1.2.0", commit.clone(), tagger.clone());

        assert_eq!(request.tag, "v1.2.0");
        assert_eq!(request.message, "Version 1.2.0");
        assert_eq!(request.commit, commit);
        assert_eq!(request.tagger, tagger);
    }
}
