use serde::Deserialize;

/// What GitHub sets on a row it lists among issues that is really a pull
/// request: an object of links to the pull request, none of which is read,
/// so that only an object, never `null`, marks a row as one.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubIssuePullRequest {}
