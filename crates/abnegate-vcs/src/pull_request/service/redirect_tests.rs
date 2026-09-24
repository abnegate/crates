use super::*;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::seven;
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

/// Every status a server redirects a request with.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// The methods a request that is not a read goes out with.
const WRITES: [Method; 3] = [Method::POST, Method::PUT, Method::DELETE];

/// Where the redirects a test answers with point, one address per status.
const TARGETS: &str = "/to";

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

#[tokio::test]
async fn a_redirect_within_the_origin_is_not_followed_out_of_it() {
    let origin = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(moved(format!("{}/repositories/42", origin.uri())))
        .expect(1)
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path("/repositories/42"))
        .respond_with(moved(format!("{}{REPOSITORY}", elsewhere.uri())))
        .expect(1)
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

    assert!(
        elsewhere.received_requests().await.unwrap().is_empty(),
        "a hop within the origin does not make the next one out of it followable"
    );
    assert!(
        matches!(result, Err(PullRequestError::GitHubApi(_))),
        "the unfollowed redirect is an answer GitHub gave: {result:?}"
    );
}

/// A POST GitHub answers with a 302 or 303 is not followed to a GET of
/// wherever the redirect points, whose answer would say nothing about the
/// comment or the mutation that was asked for.
#[tokio::test]
async fn a_redirected_post_is_not_taken_for_its_answer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/project/issues/7/comments"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/elsewhere", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(303)
                .insert_header("location", format!("{}/elsewhere", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": 5,
            "data": { "resolveReviewThread": { "thread": { "isResolved": true } } },
        })))
        .expect(0)
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let posted = service
        .post_issue_comment(&seven(&service), &token(), "@review-bot review")
        .await;
    let resolved = service.resolve_review_thread("PRRT_1", &token()).await;

    for answer in [posted.map(|_| ()), resolved] {
        assert!(
            matches!(answer, Err(PullRequestError::GitHubApi(ref text)) if text == REDIRECTED),
            "a POST answered by a GET did nothing it was asked to: {answer:?}"
        );
    }
}

/// GitHub answers a write to a renamed or transferred repository with a
/// redirect to its new address. A 307 or 308 keeps any method and its body,
/// and a 301 or 302 keeps every method but POST, so following one would carry
/// the write out there before its answer could be refused. A write is refused
/// where it was sent and never sent on, whatever answers at the target; a read
/// still follows.
#[tokio::test]
async fn a_redirected_write_is_refused_and_never_sent_on() {
    let server = MockServer::start().await;
    for redirect in REDIRECTS {
        for (target, status) in [("found", 200), ("missing", 404)] {
            let to = format!("{TARGETS}/{redirect}/{target}");
            Mock::given(method("GET"))
                .and(path(&to))
                .respond_with(ResponseTemplate::new(status).set_body_json(serde_json::json!({})))
                .mount(&server)
                .await;
            for write in WRITES {
                Mock::given(method(write.as_str()))
                    .and(path(&to))
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                    .expect(0)
                    .mount(&server)
                    .await;
            }
            Mock::given(path(format!("/{redirect}/{target}")))
                .respond_with(
                    ResponseTemplate::new(redirect)
                        .insert_header("location", format!("{}{to}", server.uri())),
                )
                .mount(&server)
                .await;
        }
    }
    Mock::given(path("/direct"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;
    let address = |route: &str| Url::parse(&format!("{}{route}", server.uri())).unwrap();

    for redirect in REDIRECTS {
        for target in ["found", "missing"] {
            for write in WRITES {
                let answer = answered(
                    service
                        .request(
                            write.clone(),
                            address(&format!("/{redirect}/{target}")),
                            &token(),
                            ACCEPT,
                        )
                        .json(&serde_json::json!({ "body": "sent once" })),
                )
                .await;

                assert!(
                    matches!(answer, Err(PullRequestError::GitHubApi(ref text)) if text == REDIRECTED),
                    "{write} {redirect} to {target}: {answer:?}"
                );
            }
        }

        let read = answered(service.request(
            Method::GET,
            address(&format!("/{redirect}/found")),
            &token(),
            ACCEPT,
        ))
        .await;
        assert!(read.is_ok(), "GET {redirect}: {read:?}");
    }

    let carried: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.method != Method::GET && request.url.path().starts_with(TARGETS))
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    assert!(
        carried.is_empty(),
        "a redirected write was carried out where the redirect pointed: {carried:?}"
    );

    for write in WRITES {
        let answer =
            answered(service.request(write.clone(), address("/direct"), &token(), ACCEPT)).await;
        assert!(answer.is_ok(), "{write} unredirected: {answer:?}");
    }
}
