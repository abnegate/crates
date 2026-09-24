use super::*;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use reqwest::header::AUTHORIZATION;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// The path GitHub reads a repository's details from.
const REPOSITORY: &str = "/repos/acme/project";

fn moved(location: String) -> ResponseTemplate {
    ResponseTemplate::new(301).insert_header("location", location)
}

fn trunk() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "default_branch": "trunk" }))
}

#[tokio::test]
async fn a_redirect_to_another_origin_is_returned_rather_than_followed() {
    let origin = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(moved(format!("{}{REPOSITORY}", elsewhere.uri())))
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .respond_with(trunk())
        .mount(&elsewhere)
        .await;
    let service = stand_in(&origin).await;

    let result = service
        .get_default_branch(&project(&service), &token())
        .await;

    let received = elsewhere.received_requests().await.unwrap();
    assert!(
        received
            .iter()
            .all(|request| !request.headers.contains_key(AUTHORIZATION)),
        "the token must never reach another origin"
    );
    assert!(
        received.is_empty(),
        "a redirect to another origin must not be followed"
    );
    assert!(
        matches!(result, Err(PullRequestError::GitHubApi(_))),
        "the unfollowed redirect is an answer GitHub gave: {result:?}"
    );
}

/// GitHub answers for a renamed or transferred repository with a 301 to its
/// numeric address on the same origin.
#[tokio::test]
async fn a_redirect_within_the_origin_is_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(moved(format!("{}/repositories/42", server.uri())))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repositories/42"))
        .and(header("authorization", "Bearer token"))
        .respond_with(trunk())
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    assert_eq!(
        service
            .get_default_branch(&project(&service), &token())
            .await
            .unwrap()
            .as_str(),
        "trunk"
    );
}

#[tokio::test]
async fn a_redirect_loop_within_the_origin_stops_after_ten_hops() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(moved(format!("{}{REPOSITORY}", server.uri())))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let result = service
        .get_default_branch(&project(&service), &token())
        .await;

    assert!(
        matches!(result, Err(PullRequestError::GitHubApi(_))),
        "the last unfollowed redirect is an answer GitHub gave: {result:?}"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        11,
        "the first request and ten redirects, and no more"
    );
}
