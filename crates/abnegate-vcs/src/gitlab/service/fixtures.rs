use super::*;
use crate::object_id::ObjectId;
use wiremock::MockServer;

/// The path GitLab serves its REST API under.
pub(super) const API: &str = "/api/v4";

/// A service addressing `server`, a mock GitLab on a loopback port.
pub(super) fn served(server: &MockServer) -> GitLabService {
    GitLabService::configured_allowing_http(&server.uri()).unwrap()
}

/// The token every test request carries unless it reads anonymously.
pub(super) fn token() -> SecretValue {
    SecretValue::new("token")
}

/// An empty token, which only an anonymous read may send without.
pub(super) fn empty() -> SecretValue {
    SecretValue::new("")
}

/// The project the tests read, named by a path with a `/` in it.
pub(super) fn project() -> Project {
    Project::parse("acme/project").unwrap()
}

/// The object named by forty copies of `digit`.
pub(super) fn object(digit: char) -> ObjectId {
    ObjectId::parse(&digit.to_string().repeat(40)).unwrap()
}
