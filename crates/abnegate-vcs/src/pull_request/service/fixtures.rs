use super::*;
use crate::commit_sha::CommitSha;
use wiremock::Match;
use wiremock::MockServer;

/// Whether an error is the one a refusal has to be reported as.
pub(super) type Expected = fn(&PullRequestError) -> bool;

/// Matches a request whose query holds exactly these parameters, each once,
/// in any order, and nothing else.
pub(super) struct ExactQuery(Vec<(String, String)>);

impl Match for ExactQuery {
    fn matches(&self, request: &wiremock::Request) -> bool {
        let mut sent: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
        sent.sort();
        sent == self.0
    }
}

/// A matcher for a request whose query is `pairs` and nothing else.
pub(super) fn query_is(pairs: &[(&str, &str)]) -> ExactQuery {
    let mut expected: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    expected.sort();
    ExactQuery(expected)
}

pub(super) fn token() -> SecretValue {
    SecretValue::new("token")
}

pub(super) async fn stand_in(server: &MockServer) -> PullRequestService {
    PullRequestService::standing_in_for("github.com", &server.uri()).unwrap()
}

pub(super) fn seven(service: &PullRequestService) -> PullRequestReference {
    service
        .pull_request("https://github.com/acme/project/pull/7")
        .unwrap()
}

pub(super) fn project(service: &PullRequestService) -> Repository {
    service
        .parse_github_url("https://github.com/acme/project")
        .unwrap()
}

pub(super) fn commit(digit: char) -> CommitSha {
    CommitSha::parse(&digit.to_string().repeat(40)).unwrap()
}
