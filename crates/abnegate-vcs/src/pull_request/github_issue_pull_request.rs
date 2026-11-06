use serde::Deserialize;
use serde::Deserializer;
use serde::de::IgnoredAny;
use std::collections::BTreeMap;

/// What GitHub sets on a row it lists among issues that is really a pull
/// request: an object of links to the pull request, none of which is read.
///
/// Only an object marks a row as one. `null` is no marker, and anything else
/// in its place, an array among them, is malformed.
#[derive(Debug, Clone)]
pub(super) struct GitHubIssuePullRequest;

impl<'de> Deserialize<'de> for GitHubIssuePullRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<String, IgnoredAny>::deserialize(deserializer).map(|_| Self)
    }
}
