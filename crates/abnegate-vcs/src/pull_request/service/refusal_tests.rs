use super::*;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::seven;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// The path GitHub reads a repository's details from.
const REPOSITORY: &str = "/repos/acme/project";

/// The path GitHub opens a pull request at.
const PULLS: &str = "/repos/acme/project/pulls";

/// A token shaped as GitHub's own, which the redactor knows to hide.
const SHAPED_TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

/// A token shaped as nothing the redactor knows, so only never reading the
/// request keeps it out of an error.
const PLAIN_TOKEN: &str = "request-token-SENTINEL";

/// The error a read of the repository's default branch is answered with,
/// given `response`.
async fn reading(response: ResponseTemplate) -> PullRequestError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;
    let service = stand_in(&server).await;
    service
        .get_default_branch(&project(&service), &token())
        .await
        .expect_err("a refusal is not a branch")
}

/// The error opening a pull request is answered with, given `response`.
async fn opening(response: ResponseTemplate) -> PullRequestError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PULLS))
        .respond_with(response)
        .expect(1)
        .mount(&server)
        .await;
    let service = stand_in(&server).await;
    service
        .create_pull_request(
            &project(&service),
            &token(),
            &BranchName::parse("feature").unwrap(),
            &BranchName::parse("main").unwrap(),
            "title",
            "body",
            false,
        )
        .await
        .expect_err("a refusal is not a pull request")
}

fn refused_with(failure: &PullRequestError, status: u16, message: &str) -> bool {
    matches!(
        failure,
        PullRequestError::Refused { status: said, message: words }
            if *said == status && words == message
    )
}

#[tokio::test]
async fn a_redirect_a_read_does_not_follow_is_refused_with_its_status() {
    let failure = reading(
        ResponseTemplate::new(302)
            .insert_header("location", "https://elsewhere.invalid/repos/acme/project")
            .set_body_json(json!({ "message": "Moved elsewhere" })),
    )
    .await;
    assert!(
        refused_with(&failure, 302, "Moved elsewhere"),
        "{failure:?}"
    );
    assert_eq!(
        failure.to_string(),
        "GitHub refused the request with status 302: Moved elsewhere"
    );

    let failure = reading(ResponseTemplate::new(304)).await;
    assert!(refused_with(&failure, 304, ""), "{failure:?}");
    assert_eq!(
        failure.to_string(),
        "GitHub refused the request with status 304"
    );
}

#[tokio::test]
async fn a_conflict_no_method_reads_for_itself_is_refused_with_its_status() {
    let failure = opening(
        ResponseTemplate::new(409).set_body_json(json!({ "message": "Git Repository is empty." })),
    )
    .await;

    assert!(
        refused_with(&failure, 409, "Git Repository is empty."),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_422_that_is_not_a_duplicate_is_refused_with_github_s_words() {
    let failure = opening(ResponseTemplate::new(422).set_body_json(json!({
        "message": "Validation Failed",
        "errors": [{
            "resource": "PullRequest",
            "code": "custom",
            "message": "No commits between main and feature",
        }],
        "documentation_url": "https://docs.github.com/rest",
    })))
    .await;

    assert!(
        refused_with(
            &failure,
            422,
            "Validation Failed; No commits between main and feature"
        ),
        "{failure:?}"
    );

    let duplicate = opening(ResponseTemplate::new(422).set_body_json(json!({
        "message": "Validation Failed",
        "errors": [{ "message": "A pull request already exists for acme:feature." }],
    })))
    .await;
    assert!(
        matches!(duplicate, PullRequestError::PullRequestAlreadyExists(_)),
        "{duplicate:?}"
    );
}

/// No status but a rate limit is sent again, so a failure of GitHub's own is
/// reported once, as it was answered, even by a service that waits out rate
/// limits.
#[tokio::test]
async fn a_failure_of_github_s_own_is_refused_with_its_status_and_never_sent_again() {
    for (status, message) in [(500, "Server Error"), (503, "Service Unavailable")] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(REPOSITORY))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("retry-after", "0")
                    .set_body_json(json!({ "message": message })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server)
            .await
            .with_retry_after(Duration::from_secs(1));

        let failure = service
            .get_default_branch(&project(&service), &token())
            .await
            .unwrap_err();

        assert!(refused_with(&failure, status, message), "{failure:?}");
        assert_eq!(
            failure.to_string(),
            format!("GitHub refused the request with status {status}: {message}")
        );
    }
}

#[tokio::test]
async fn an_answer_without_github_s_words_is_refused_with_its_start() {
    let failure = reading(
        ResponseTemplate::new(502)
            .set_body_string("<html>\n  <body>\n    <h1>502 Bad Gateway</h1>\n  </body>\n</html>"),
    )
    .await;

    assert!(
        refused_with(
            &failure,
            502,
            "<html> <body> <h1>502 Bad Gateway</h1> </body> </html>"
        ),
        "{failure:?}"
    );
}

#[tokio::test]
async fn the_start_of_a_long_answer_is_cut_to_its_bound_on_a_character() {
    let failure = reading(ResponseTemplate::new(500).set_body_string("a".repeat(10_000))).await;
    assert!(
        refused_with(&failure, 500, &"a".repeat(MAXIMUM_EXCERPT_BYTES)),
        "{failure:?}"
    );

    let wide = format!("a{}", "é".repeat(10_000));
    let failure = reading(ResponseTemplate::new(500).set_body_string(wide)).await;
    let cut = format!("a{}", "é".repeat((MAXIMUM_EXCERPT_BYTES - 1) / 2));
    assert!(refused_with(&failure, 500, &cut), "{failure:?}");
    assert!(cut.len() <= MAXIMUM_EXCERPT_BYTES, "{}", cut.len());
}

#[tokio::test]
async fn an_empty_answer_is_refused_with_no_words() {
    let failure = reading(ResponseTemplate::new(503)).await;

    assert!(refused_with(&failure, 503, ""), "{failure:?}");
    assert_eq!(
        failure.to_string(),
        "GitHub refused the request with status 503"
    );
}

/// A refusal is built from GitHub's answer alone: the token a request carried
/// never reaches the error, and one GitHub echoes back is redacted.
#[tokio::test]
async fn a_refusal_never_carries_the_token_the_request_was_sent_with() {
    for sent in [SHAPED_TOKEN, PLAIN_TOKEN] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(500).set_body_json(json!({ "message": "Server Error" })),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_pull(&seven(&service), &SecretValue::new(sent))
            .await
            .unwrap_err();

        assert!(refused_with(&failure, 500, "Server Error"), "{failure:?}");
        let rendered = format!("{failure} {failure:?}");
        assert!(!rendered.contains(sent), "{rendered}");
    }

    for body in [
        json!({ "message": format!("Bad credentials: {SHAPED_TOKEN}") }).to_string(),
        format!("<p>Bad credentials: {SHAPED_TOKEN}</p>"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_body_string(body.clone()))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_pull(&seven(&service), &SecretValue::new(SHAPED_TOKEN))
            .await
            .unwrap_err();

        let rendered = format!("{failure} {failure:?}");
        assert!(!rendered.contains(SHAPED_TOKEN), "{body}: {rendered}");
        assert!(rendered.contains("[REDACTED]"), "{body}: {rendered}");
    }
}
