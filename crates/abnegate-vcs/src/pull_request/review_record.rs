use crate::commit_sha::CommitSha;
use serde::Deserialize;
use serde::Serialize;

/// A review submitted on a pull request, and the commit it was submitted on,
/// where a review bot on a paid plan posts its score.
///
/// Written and read under GitHub's own names for a review's commit and time,
/// `commit_id` and `submitted_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReviewRecord {
    /// GitHub's identifier for the review.
    pub id: u64,
    /// The login of whoever submitted it, or empty when GitHub does not say.
    pub author: String,
    /// What it says, in Markdown, or empty when it says nothing.
    pub body: String,
    /// The pull request's head when it was submitted, if GitHub says.
    #[serde(rename = "commit_id", default)]
    pub commit: Option<CommitSha>,
    /// When it was submitted, as RFC 3339, or `None` while it is pending.
    #[serde(default)]
    pub submitted_at: Option<String>,
}

impl ReviewRecord {
    /// The review GitHub identifies by `id`, submitted by `author` and saying
    /// `body`, on no commit and at no time GitHub says.
    pub fn new(id: u64, author: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id,
            author: author.into(),
            body: body.into(),
            commit: None,
            submitted_at: None,
        }
    }

    /// The pull request's head when it was submitted.
    #[must_use]
    pub fn with_commit(mut self, commit: CommitSha) -> Self {
        self.commit = Some(commit);
        self
    }

    /// When it was submitted, as RFC 3339.
    #[must_use]
    pub fn with_submitted_at(mut self, submitted_at: impl Into<String>) -> Self {
        self.submitted_at = Some(submitted_at.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn head() -> CommitSha {
        CommitSha::parse("39edddf0d1abc06b84ebb74e8213c1fd41ec333e").unwrap()
    }

    fn scored() -> ReviewRecord {
        ReviewRecord::new(
            5_333_502_514,
            "coderabbitai[bot]",
            "**Actionable comments posted: 1**",
        )
        .with_commit(head())
        .with_submitted_at("2026-09-28T02:51:09Z")
    }

    #[test]
    fn a_review_keeps_github_s_names_for_its_commit_and_its_time() {
        let written = serde_json::to_value(scored()).unwrap();

        assert_eq!(
            written,
            json!({
                "id": 5_333_502_514u64,
                "author": "coderabbitai[bot]",
                "body": "**Actionable comments posted: 1**",
                "commit_id": "39edddf0d1abc06b84ebb74e8213c1fd41ec333e",
                "submitted_at": "2026-09-28T02:51:09Z",
            })
        );
        assert_eq!(
            serde_json::from_value::<ReviewRecord>(written).unwrap(),
            scored()
        );
    }

    #[test]
    fn a_pending_review_on_no_commit_reads_without_either() {
        let read: ReviewRecord =
            serde_json::from_value(json!({ "id": 3, "author": "", "body": "" })).unwrap();

        assert_eq!(read, ReviewRecord::new(3, "", ""));
    }

    #[test]
    fn a_stored_commit_this_crate_does_not_accept_is_refused() {
        let refused = serde_json::from_value::<ReviewRecord>(json!({
            "id": 3,
            "author": "",
            "body": "",
            "commit_id": "HEAD",
        }));

        assert!(refused.is_err());
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let bare = ReviewRecord::new(1, "ada", "Looks good.");

        assert_eq!(
            bare.clone().with_commit(head()),
            ReviewRecord {
                commit: Some(head()),
                ..bare.clone()
            }
        );
        assert_eq!(
            bare.clone().with_submitted_at("2026-09-28T02:51:09Z"),
            ReviewRecord {
                submitted_at: Some("2026-09-28T02:51:09Z".to_string()),
                ..bare
            }
        );
    }
}
