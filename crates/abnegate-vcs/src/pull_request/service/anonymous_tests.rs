use super::*;
use crate::pull_request::JobLog;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::seven;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use reqwest::header::AUTHORIZATION;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

/// The path GitHub reads a repository's details from.
const REPOSITORY: &str = "/repos/acme/project";

/// Where a repository GitHub moved is read from instead, on the same origin.
const MOVED: &str = "/repositories/1";

/// The path GitHub reads job 9's log from.
const JOB_LOG: &str = "/repos/acme/project/actions/jobs/9/logs";

/// Where GitHub stores job 9's log, on the same origin.
const STORED_LOG: &str = "/storage/log";

/// The path a comment on pull request 7's conversation is posted to.
const COMMENTS: &str = "/repos/acme/project/issues/7/comments";

/// The API version the versioned tests name.
const VERSION: &str = "2022-11-28";

/// Most bytes of a log the tests read.
const LIMIT: usize = 1024;

fn empty() -> SecretValue {
    SecretValue::new("")
}

fn trunk() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "trunk" }))
}

async fn anonymous(server: &MockServer) -> PullRequestService {
    stand_in(server).await.with_anonymous_reads()
}

/// A mock that answers anything, and that no request may reach.
async fn untouched(server: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(server)
        .await;
}

/// Every request that reached `server`.
async fn arrivals(server: &MockServer) -> Vec<wiremock::Request> {
    server.received_requests().await.unwrap()
}

fn header_of<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn an_empty_tokens_read_is_sent_without_authorization_under_anonymous_reads() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(trunk())
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await.with_api_version(VERSION);

    let branch = service
        .get_default_branch(&project(&service), &empty())
        .await
        .unwrap();

    assert_eq!(branch.as_str(), "trunk");
    let received = arrivals(&server).await;
    assert!(
        !received[0].headers.contains_key(AUTHORIZATION),
        "an anonymous read carries no authorization"
    );
    assert_eq!(header_of(&received[0], API_VERSION), Some(VERSION));
    assert_eq!(header_of(&received[0], "accept"), Some(ACCEPT));
}

#[tokio::test]
async fn a_token_under_anonymous_reads_is_still_sent_as_a_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .and(header("authorization", "Bearer token"))
        .respond_with(trunk())
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await;

    let branch = service
        .get_default_branch(&project(&service), &token())
        .await
        .unwrap();

    assert_eq!(branch.as_str(), "trunk");
}

#[tokio::test]
async fn an_empty_tokens_graphql_calls_are_refused_unsent_under_anonymous_reads() {
    let server = MockServer::start().await;
    untouched(&server).await;
    let service = anonymous(&server).await;

    let collected = service
        .collect_review_threads(&seven(&service), &empty())
        .await;
    let resolved = service.resolve_review_thread("PRRT_1", &empty()).await;

    assert!(
        matches!(collected, Err(PullRequestError::AuthenticationRequired)),
        "{collected:?}"
    );
    assert!(
        matches!(resolved, Err(PullRequestError::AuthenticationRequired)),
        "{resolved:?}"
    );
}

#[tokio::test]
async fn an_empty_tokens_write_is_refused_unsent_under_anonymous_reads() {
    let server = MockServer::start().await;
    untouched(&server).await;
    let service = anonymous(&server).await;

    let posted = service
        .post_issue_comment(&seven(&service), &empty(), "hello")
        .await;

    assert!(
        matches!(posted, Err(PullRequestError::AuthenticationRequired)),
        "{posted:?}"
    );
}

#[tokio::test]
async fn an_empty_tokens_job_log_is_read_without_authorization_under_anonymous_reads() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(JOB_LOG))
        .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await;
    let job = NonZeroU64::new(9).unwrap();

    let read = service
        .fetch_job_log(&project(&service), &empty(), job, LIMIT)
        .await
        .unwrap();

    assert_eq!(read, JobLog::new(b"step one\n".to_vec(), false));
    assert!(
        !arrivals(&server).await[0]
            .headers
            .contains_key(AUTHORIZATION),
        "a job log is a read, whichever client sends it"
    );
}

#[tokio::test]
async fn an_empty_token_without_anonymous_reads_is_refused_unsent() {
    let server = MockServer::start().await;
    untouched(&server).await;
    let service = stand_in(&server).await;

    let read = service
        .get_default_branch(&project(&service), &empty())
        .await;

    assert!(
        matches!(read, Err(PullRequestError::AuthenticationFailed)),
        "an empty token must never be sent as `Bearer `: {read:?}"
    );
}

#[tokio::test]
async fn an_empty_tokens_write_and_graphql_call_without_anonymous_reads_fail_authentication() {
    let server = MockServer::start().await;
    untouched(&server).await;
    let service = stand_in(&server).await;

    let posted = service
        .post_issue_comment(&seven(&service), &empty(), "hello")
        .await;
    let resolved = service.resolve_review_thread("PRRT_1", &empty()).await;

    assert!(
        matches!(posted, Err(PullRequestError::AuthenticationFailed)),
        "{posted:?}"
    );
    assert!(
        matches!(resolved, Err(PullRequestError::AuthenticationFailed)),
        "{resolved:?}"
    );
}

#[tokio::test]
async fn an_anonymous_read_redirected_within_the_origin_carries_no_authorization_on_either_hop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", format!("{}{MOVED}", server.uri()).as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MOVED))
        .respond_with(trunk())
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await;

    let branch = service
        .get_default_branch(&project(&service), &empty())
        .await
        .unwrap();

    assert_eq!(branch.as_str(), "trunk");
    let received = arrivals(&server).await;
    assert_eq!(received.len(), 2);
    assert!(
        received
            .iter()
            .all(|request| !request.headers.contains_key(AUTHORIZATION)),
        "neither hop of an anonymous read carries authorization"
    );
}

#[tokio::test]
async fn a_token_under_anonymous_reads_is_still_sent_on_a_write_and_a_graphql_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMMENTS))
        .and(header("authorization", "Bearer token"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 42 })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "resolveReviewThread": { "thread": { "isResolved": true } } },
        })))
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await;

    let posted = service
        .post_issue_comment(&seven(&service), &token(), "hello")
        .await
        .unwrap();
    service
        .resolve_review_thread("PRRT_1", &token())
        .await
        .unwrap();

    assert_eq!(posted, 42);
}

#[tokio::test]
async fn an_anonymous_job_log_redirected_to_storage_carries_no_authorization_on_either_hop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(JOB_LOG))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("{}{STORED_LOG}?sig=signed-secret", server.uri()).as_str(),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(STORED_LOG))
        .and(query_param("sig", "signed-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
        .expect(1)
        .mount(&server)
        .await;
    let service = anonymous(&server).await;
    let job = NonZeroU64::new(9).unwrap();

    let read = service
        .fetch_job_log(&project(&service), &empty(), job, LIMIT)
        .await
        .unwrap();

    assert_eq!(read, JobLog::new(b"step one\n".to_vec(), false));
    let received = arrivals(&server).await;
    assert_eq!(received.len(), 2);
    assert!(
        received
            .iter()
            .all(|request| !request.headers.contains_key(AUTHORIZATION)),
        "neither the request for the log nor its download carries authorization"
    );
}

#[tokio::test]
async fn an_empty_tokens_job_log_without_anonymous_reads_is_refused_unsent() {
    let server = MockServer::start().await;
    untouched(&server).await;
    let service = stand_in(&server).await;
    let job = NonZeroU64::new(9).unwrap();

    let read = service
        .fetch_job_log(&project(&service), &empty(), job, LIMIT)
        .await;

    assert!(
        matches!(read, Err(PullRequestError::AuthenticationFailed)),
        "{read:?}"
    );
}
