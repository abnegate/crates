use super::*;
use crate::commit_sha::CommitSha;
use wiremock::MockServer;

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
