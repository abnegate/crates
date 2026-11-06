use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::IssueReference;
use crate::pull_request::service::fixtures::Expected;
use crate::pull_request::service::fixtures::commit;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::seven;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use reqwest::Body;
use serde_json::Value;
use serde_json::json;
use std::time::UNIX_EPOCH;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// The commit `main` is resolved through.
const COMMIT: &str = "/repos/acme/project/commits/main";

/// Where a pull request is opened.
const PULLS: &str = "/repos/acme/project/pulls";

/// The longest wait the services in these tests are built to take.
const CEILING: Duration = Duration::from_secs(60);

/// The API version every request in the test that names one carries.
const VERSION: &str = "2026-03-10";

/// How an HTTP-date is written in `Retry-After`.
const HTTP_DATE: &str = "%a, %d %b %Y %H:%M:%S GMT";

/// When the window in these tests ends, as seconds since the Unix epoch.
const RESET_SECONDS: u64 = 1_700_000_000;

/// How long a rate limit that spent the window asks to be waited on.
const SPENT_WAIT: Duration = Duration::from_secs(3);

/// How long a rate limit worth waiting on asks to be waited on.
const SHORT_WAIT: Duration = Duration::from_secs(1);

/// A service that waits once on a rate limit that asks for no more than a
/// minute.
async fn retrying(server: &MockServer) -> PullRequestService {
    stand_in(server).await.with_retry_after(CEILING)
}

/// `main` answered once with `answer`, and with a commit after that.
async fn rate_limited(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(COMMIT))
        .respond_with(answer)
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(COMMIT))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": commit('a').as_str() })),
        )
        .mount(server)
        .await;
}

async fn resolve(service: &PullRequestService) -> PullRequestResult<CommitSha> {
    service
        .resolve_commit(&project(service), &token(), "main")
        .await
}

/// How many requests reached `server`.
async fn requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("wiremock to record requests")
        .len()
}

/// A rate limit that asks for a wait of `retry_after`.
fn waiting(status: u16, retry_after: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).insert_header("retry-after", retry_after)
}

/// A rate limit that spent the window GitHub describes in full.
fn spent(status: u16) -> ResponseTemplate {
    waiting(status, &SPENT_WAIT.as_secs().to_string())
        .insert_header("x-ratelimit-limit", "5000")
        .insert_header("x-ratelimit-remaining", "0")
        .insert_header("x-ratelimit-used", "5000")
        .insert_header("x-ratelimit-reset", RESET_SECONDS.to_string())
        .insert_header("x-ratelimit-resource", "core")
}

/// `seconds` from now, written as an HTTP-date, and the instant it names.
fn date_ahead(seconds: i64) -> (String, SystemTime) {
    let date = (Utc::now() + TimeDelta::seconds(seconds))
        .format(HTTP_DATE)
        .to_string();
    let at = DateTime::parse_from_rfc2822(&date).expect("an HTTP-date chrono reads");
    (date, at.into())
}

/// The pull request GitHub answers an opening with.
fn opened() -> Value {
    json!({
        "id": 1,
        "number": 12,
        "html_url": "https://github.com/acme/project/pull/12",
        "state": "open",
        "title": "title",
        "body": null,
        "head": { "ref": "feature", "sha": commit('a').as_str() },
        "base": { "ref": "main", "sha": commit('b').as_str() },
    })
}

async fn open(service: &PullRequestService) -> PullRequestResult<CreatedPullRequest> {
    service
        .create_pull_request(
            &project(service),
            &token(),
            &BranchName::parse("feature").unwrap(),
            &BranchName::parse("main").unwrap(),
            "title",
            "body",
            false,
        )
        .await
}

#[tokio::test]
async fn an_exhausted_quota_is_reported_and_never_waited_on() {
    let expected = RateLimit::default()
        .with_limit(5000)
        .with_remaining(0)
        .with_used(5000)
        .with_reset(UNIX_EPOCH + Duration::from_secs(RESET_SECONDS))
        .with_resource("core");
    for status in [403, 429] {
        let server = MockServer::start().await;
        rate_limited(&server, spent(status)).await;
        let service = retrying(&server).await;
        let started = Instant::now();

        let failure = resolve(&service).await.unwrap_err();

        assert!(
            matches!(&failure, PullRequestError::QuotaExhausted { rate_limit } if **rate_limit == expected),
            "{status}: {failure:?}"
        );
        assert_eq!(requests(&server).await, 1, "{status}");
        assert!(
            started.elapsed() < SPENT_WAIT,
            "{status}: an exhausted quota was waited on"
        );
    }
}

#[tokio::test]
async fn without_the_opt_in_an_exhausted_quota_is_a_rate_limit() {
    for status in [403, 429] {
        let server = MockServer::start().await;
        rate_limited(&server, spent(status)).await;
        let service = stand_in(&server).await;

        let failure = resolve(&service).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::RateLimited),
            "{status}: {failure:?}"
        );
        assert_eq!(requests(&server).await, 1, "{status}");
    }
}

#[tokio::test]
async fn without_the_opt_in_a_retry_after_is_not_waited_on() {
    let server = MockServer::start().await;
    rate_limited(&server, waiting(429, &SHORT_WAIT.as_secs().to_string())).await;
    let service = stand_in(&server).await;

    let failure = resolve(&service).await.unwrap_err();

    assert!(
        matches!(failure, PullRequestError::RateLimited),
        "{failure:?}"
    );
    assert_eq!(requests(&server).await, 1);
}

#[tokio::test]
async fn a_retry_after_in_seconds_is_waited_once() {
    for status in [403, 429] {
        let server = MockServer::start().await;
        rate_limited(&server, waiting(status, &SHORT_WAIT.as_secs().to_string())).await;
        let service = retrying(&server).await;
        let started = Instant::now();

        let resolved = resolve(&service).await;

        assert_eq!(resolved.unwrap(), commit('a'), "{status}");
        assert_eq!(requests(&server).await, 2, "{status}");
        assert!(
            started.elapsed() >= SHORT_WAIT,
            "{status}: sent again after {:?}",
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn a_retry_after_date_is_waited_until_that_instant() {
    let server = MockServer::start().await;
    let (date, at) = date_ahead(2);
    rate_limited(&server, waiting(429, &date)).await;
    let service = retrying(&server).await;

    let resolved = resolve(&service).await.unwrap();

    assert_eq!(resolved, commit('a'));
    assert_eq!(requests(&server).await, 2);
    assert!(SystemTime::now() >= at, "sent again before {date}");
}

#[tokio::test]
async fn a_retry_after_past_the_ceiling_is_a_rate_limit_at_once() {
    let ceiling = Duration::from_secs(2);
    let (date, _) = date_ahead(30);
    for retry_after in ["3", date.as_str()] {
        let server = MockServer::start().await;
        rate_limited(&server, waiting(429, retry_after)).await;
        let service = stand_in(&server).await.with_retry_after(ceiling);

        let failure = resolve(&service).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::RateLimited),
            "{retry_after}: {failure:?}"
        );
        assert_eq!(requests(&server).await, 1, "{retry_after}");
    }
}

#[tokio::test]
async fn a_retried_answer_that_is_an_exhausted_quota_is_reported() {
    let server = MockServer::start().await;
    for status in [403, 429] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(COMMIT))
            .respond_with(waiting(429, "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(COMMIT))
            .respond_with(ResponseTemplate::new(status).insert_header("x-ratelimit-remaining", "0"))
            .with_priority(2)
            .mount(&server)
            .await;
        let service = retrying(&server).await;

        let failure = resolve(&service).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::QuotaExhausted { .. }),
            "{status}: {failure:?}"
        );
        assert_eq!(requests(&server).await, 2, "{status}");
    }
}

#[tokio::test]
async fn a_rate_limit_is_waited_on_once_and_never_in_a_loop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(COMMIT))
        .respond_with(waiting(429, "0"))
        .mount(&server)
        .await;
    let service = retrying(&server).await;

    let failure = resolve(&service).await.unwrap_err();

    assert!(
        matches!(failure, PullRequestError::RateLimited),
        "{failure:?}"
    );
    assert_eq!(requests(&server).await, 2);
}

#[tokio::test]
async fn a_plain_forbidden_answer_is_forbidden_and_sent_once() {
    let server = MockServer::start().await;
    rate_limited(&server, ResponseTemplate::new(403)).await;
    let service = retrying(&server).await;

    let failure = resolve(&service).await.unwrap_err();

    assert!(
        matches!(failure, PullRequestError::Forbidden),
        "{failure:?}"
    );
    assert_eq!(requests(&server).await, 1);
}

#[tokio::test]
async fn a_write_is_sent_again_with_its_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PULLS))
        .respond_with(waiting(429, "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(PULLS))
        .respond_with(ResponseTemplate::new(201).set_body_json(opened()))
        .mount(&server)
        .await;
    let service = retrying(&server).await;

    let created = open(&service).await.unwrap();

    assert_eq!(created.number.get(), 12);
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(bodies[0]["head"], "feature");
    assert_eq!(bodies[0]["base"], "main");
}

#[tokio::test]
async fn a_body_that_can_be_sent_only_once_is_not_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/stream"))
        .respond_with(waiting(429, "0"))
        .mount(&server)
        .await;
    let service = retrying(&server).await;
    let url = Url::parse(&format!("{}/stream", server.uri())).unwrap();

    let answer = service
        .sent(
            service
                .request(Method::PUT, url, &token(), ACCEPT)
                .unwrap()
                .body(Body::wrap(String::from("sent once"))),
        )
        .await
        .unwrap();

    assert_eq!(answer.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(requests(&server).await, 1);
}

#[tokio::test]
async fn every_kind_of_request_waits_once_under_its_api_version() {
    let server = MockServer::start().await;
    let routes = [
        (
            "GET",
            "/repos/acme/project",
            json!({ "default_branch": "main" }),
        ),
        ("GET", "/repos/acme/project/pulls/7/reviews", json!([])),
        ("PATCH", "/repos/acme/project/issues/5", json!({})),
        ("POST", PULLS, opened()),
        ("POST", "/graphql", json!({ "data": {} })),
        (
            "GET",
            "/repos/acme/project/actions/jobs/9/logs",
            json!("log"),
        ),
        ("GET", COMMIT, json!({ "sha": commit('a').as_str() })),
    ];
    for (verb, route, answer) in routes {
        Mock::given(method(verb))
            .and(path(route))
            .and(header(API_VERSION, VERSION))
            .respond_with(waiting(429, "0"))
            .up_to_n_times(1)
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method(verb))
            .and(path(route))
            .and(header(API_VERSION, VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(1)
            .mount(&server)
            .await;
    }
    let service = retrying(&server).await.with_api_version(VERSION);
    let repository = project(&service);
    let issue = IssueReference::new(repository.clone(), NonZeroU64::new(5).unwrap());

    service
        .get_default_branch(&repository, &token())
        .await
        .expect("a read");
    service
        .fetch_reviews(&seven(&service), &token())
        .await
        .expect("a paged read");
    service
        .close_issue(&issue, &token())
        .await
        .expect("a JSON write");
    open(&service).await.expect("an opening");
    service
        .resolve_review_thread("PRRT_1", &token())
        .await
        .expect("a GraphQL call");
    service
        .fetch_job_log(&repository, &token(), NonZeroU64::new(9).unwrap(), 64)
        .await
        .expect("a job's log");
    resolve(&service).await.expect("a resolved commit");
}

#[tokio::test]
async fn only_a_rate_limit_with_a_readable_delay_is_waited_on() {
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1;
    let answers: [(ResponseTemplate, Expected); 3] = [
        (
            ResponseTemplate::new(429)
                .insert_header("x-ratelimit-remaining", "10")
                .insert_header("x-ratelimit-reset", reset.to_string()),
            |failure| matches!(failure, PullRequestError::RateLimited),
        ),
        (waiting(429, "soon"), |failure| {
            matches!(failure, PullRequestError::RateLimited)
        }),
        (waiting(503, "0"), |failure| {
            matches!(failure, PullRequestError::GitHubApi(_))
        }),
    ];
    for (answer, expected) in answers {
        let server = MockServer::start().await;
        rate_limited(&server, answer).await;
        let service = retrying(&server).await;

        let failure = resolve(&service).await.unwrap_err();

        assert!(expected(&failure), "{failure:?}");
        assert_eq!(requests(&server).await, 1, "{failure:?}");
    }
}

#[tokio::test]
async fn a_success_that_spends_the_last_request_is_a_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(COMMIT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-remaining", "0")
                .set_body_json(json!({ "sha": commit('a').as_str() })),
        )
        .mount(&server)
        .await;
    let service = retrying(&server).await;

    assert_eq!(resolve(&service).await.unwrap(), commit('a'));
    assert_eq!(requests(&server).await, 1);
}
