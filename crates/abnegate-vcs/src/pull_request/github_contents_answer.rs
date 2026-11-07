use crate::pull_request::github_contents::GitHubContents;
use serde::Deserialize;
use serde::de::IgnoredAny;

/// What GitHub's contents API answers for a path: a list of entries where the
/// path names a directory, or one file's description.
///
/// A listing is tried first, so an array is never read as a file, and its
/// entries are skipped rather than held.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum GitHubContentsAnswer {
    Listing(
        #[expect(
            dead_code,
            reason = "an array alone tells a directory's listing from a file"
        )]
        Vec<IgnoredAny>,
    ),
    File(GitHubContents),
}
