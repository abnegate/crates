use super::*;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::seven;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use reqwest::Body;
use reqwest::header::AUTHORIZATION;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// The path GitHub reads a repository's details from.
const REPOSITORY: &str = "/repos/acme/project";

/// Every status a server redirects a request with.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// The redirects that keep a request's method and body, on which a write that
/// stays on the origin is sent again whole.
const KEEPING: [u16; 2] = [307, 308];

/// The redirects that may turn a request into a GET or drop its body, on which
/// a write is refused.
const CHANGING: [u16; 3] = [301, 302, 303];

/// The methods a request that is not a read goes out with.
const WRITES: [Method; 3] = [Method::POST, Method::PUT, Method::DELETE];

/// Where the redirects a test answers with point, one address per status.
const TARGETS: &str = "/to";

fn redirecting(status: u16, location: impl AsRef<str>) -> ResponseTemplate {
    ResponseTemplate::new(status).insert_header("location", location.as_ref())
}

fn trunk() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "trunk" }))
}

/// What every write a test sends carries.
fn payload() -> Value {
    json!({ "body": "sent once" })
}

fn address(server: &MockServer, route: &str) -> Url {
    Url::parse(&format!("{}{route}", server.uri())).unwrap()
}

/// `write` sent to `url` with [`payload`], as every write to GitHub goes out.
async fn writing(
    service: &PullRequestService,
    write: Method,
    url: Url,
) -> PullRequestResult<Response> {
    answered(
        service
            .request(write, url, &token(), ACCEPT)
            .json(&payload()),
    )
    .await
}

/// Every request that reached `route` on `server`.
async fn arrivals(server: &MockServer, route: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == route)
        .collect()
}

fn header_of<'a>(request: &'a wiremock::Request, name: &HeaderName) -> Option<&'a str> {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}

fn refused<T>(answer: &PullRequestResult<T>) -> bool {
    matches!(answer, Err(PullRequestError::GitHubApi(text)) if text == REDIRECTED)
}

#[tokio::test]
async fn a_redirect_to_another_origin_is_returned_rather_than_followed() {
    let origin = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPOSITORY))
        .respond_with(redirecting(301, format!("{}{REPOSITORY}", elsewhere.uri())))
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
        .respond_with(redirecting(
            301,
            format!("{}/repositories/42", server.uri()),
        ))
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
        .respond_with(redirecting(301, format!("{}{REPOSITORY}", server.uri())))
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
        .respond_with(redirecting(
            301,
            format!("{}/repositories/42", origin.uri()),
        ))
        .expect(1)
        .mount(&origin)
        .await;
    Mock::given(method("GET"))
        .and(path("/repositories/42"))
        .respond_with(redirecting(301, format!("{}{REPOSITORY}", elsewhere.uri())))
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
        .respond_with(redirecting(302, format!("{}/elsewhere", server.uri())))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(redirecting(303, format!("{}/elsewhere", server.uri())))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/elsewhere"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
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
            refused(&answer),
            "a POST answered by a GET did nothing it was asked to: {answer:?}"
        );
    }
}

/// A 301 or 302 re-sends every method but POST without saying it kept the
/// body, and a 303, or a 301 or 302 to a POST, turns the write into a GET of
/// wherever the redirect points. A write so redirected is refused where it
/// was sent and never sent on, whatever answers at the target; a read still
/// follows every redirect within the origin.
#[tokio::test]
async fn a_write_redirected_by_a_status_that_may_change_it_is_refused_and_never_sent_on() {
    let server = MockServer::start().await;
    for redirect in REDIRECTS {
        for (target, status) in [("found", 200), ("missing", 404)] {
            let to = format!("{TARGETS}/{redirect}/{target}");
            Mock::given(method("GET"))
                .and(path(&to))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
                .mount(&server)
                .await;
            for write in WRITES {
                Mock::given(method(write.as_str()))
                    .and(path(&to))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                    .expect(0)
                    .mount(&server)
                    .await;
            }
            Mock::given(path(format!("/{redirect}/{target}")))
                .respond_with(redirecting(redirect, format!("{}{to}", server.uri())))
                .mount(&server)
                .await;
        }
    }
    Mock::given(path("/direct"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    for redirect in CHANGING {
        for target in ["found", "missing"] {
            for write in WRITES {
                let answer = writing(
                    &service,
                    write.clone(),
                    address(&server, &format!("/{redirect}/{target}")),
                )
                .await;

                assert!(
                    refused(&answer),
                    "{write} {redirect} to {target}: {answer:?}"
                );
            }
        }
    }

    for redirect in REDIRECTS {
        let read = answered(service.request(
            Method::GET,
            address(&server, &format!("/{redirect}/found")),
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
        let answer = writing(&service, write.clone(), address(&server, "/direct")).await;
        assert!(answer.is_ok(), "{write} unredirected: {answer:?}");
    }
}

/// GitHub answers a write to a renamed or transferred repository with a 307
/// or 308 to its new address on the same origin, which keeps the method and
/// the body. The write is sent there once, with its body, its token and its
/// type, and what answers there is its answer.
#[tokio::test]
async fn a_write_a_307_or_308_sends_elsewhere_on_the_origin_arrives_there_once_and_whole() {
    let server = MockServer::start().await;
    for status in KEEPING {
        for write in WRITES {
            let from = format!("/{status}/{write}");
            let to = format!("{TARGETS}{from}");
            Mock::given(path(&from))
                .respond_with(redirecting(status, format!("{}{to}", server.uri())))
                .mount(&server)
                .await;
            Mock::given(path(&to))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "at": to })))
                .mount(&server)
                .await;
        }
    }
    let service = stand_in(&server).await;

    for status in KEEPING {
        for write in WRITES {
            let from = format!("/{status}/{write}");
            let to = format!("{TARGETS}{from}");

            let answer = writing(&service, write.clone(), address(&server, &from))
                .await
                .unwrap_or_else(|failure| panic!("{write} {status}: {failure:?}"));

            let answer: Value = decode(answer).await.unwrap();
            assert_eq!(answer, json!({ "at": to }), "{write} {status}");
            let landed = arrivals(&server, &to).await;
            assert_eq!(landed.len(), 1, "{write} {status} lands at the target once");
            assert_eq!(landed[0].method, write, "{status} keeps the method");
            assert_eq!(
                landed[0].body_json::<Value>().unwrap(),
                payload(),
                "{write} {status} keeps the body"
            );
            assert_eq!(
                header_of(&landed[0], &AUTHORIZATION),
                Some("Bearer token"),
                "{write} {status} keeps the token within the origin"
            );
            assert_eq!(
                header_of(&landed[0], &CONTENT_TYPE),
                Some("application/json"),
                "{write} {status} keeps the body's type"
            );
            assert_eq!(arrivals(&server, &from).await.len(), 1, "{write} {status}");
        }
    }
}

/// Another origin is another port, another host name, even one that reaches
/// the same server, or another scheme.
#[tokio::test]
async fn a_write_redirected_to_another_origin_is_refused_and_never_sent_there() {
    let origin = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&elsewhere)
        .await;
    Mock::given(path(TARGETS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&origin)
        .await;
    let port = origin.address().port();
    let destinations = [
        format!("{}{TARGETS}", elsewhere.uri()),
        format!("http://localhost:{port}{TARGETS}"),
        format!("https://127.0.0.1:{port}{TARGETS}"),
    ];
    for status in KEEPING {
        for (index, destination) in destinations.iter().enumerate() {
            Mock::given(path(format!("/{status}/{index}")))
                .respond_with(redirecting(status, destination))
                .mount(&origin)
                .await;
        }
    }
    let service = stand_in(&origin).await;

    for status in KEEPING {
        for (index, destination) in destinations.iter().enumerate() {
            for write in WRITES {
                let answer = writing(
                    &service,
                    write.clone(),
                    address(&origin, &format!("/{status}/{index}")),
                )
                .await;

                assert!(
                    refused(&answer),
                    "{write} {status} to {destination}: {answer:?}"
                );
            }
        }
    }

    assert!(
        elsewhere.received_requests().await.unwrap().is_empty(),
        "a write, and its token, never reach another origin"
    );
    assert!(
        arrivals(&origin, TARGETS).await.is_empty(),
        "a write never reaches the origin's server under another host name or scheme"
    );
}

#[tokio::test]
async fn a_write_redirected_in_a_loop_within_the_origin_is_refused_after_ten_hops() {
    for status in KEEPING {
        let server = MockServer::start().await;
        Mock::given(path("/loop"))
            .respond_with(redirecting(status, format!("{}/loop", server.uri())))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let answer = writing(&service, Method::PUT, address(&server, "/loop")).await;

        assert!(refused(&answer), "{status}: {answer:?}");
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            MAXIMUM_REDIRECTS + 1,
            "{status}: the first request and ten sent again, and no more"
        );
    }
}

/// A `Location` that names no origin is read against the address the write
/// was last sent to, not the one it was first sent to.
#[tokio::test]
async fn a_relative_location_is_resolved_against_the_address_last_sent_to() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/start"))
        .respond_with(redirecting(308, "/moved/first"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/moved/first"))
        .respond_with(redirecting(307, "second"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/moved/second"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 1 })))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let answer = writing(&service, Method::POST, address(&server, "/start")).await;

    assert!(answer.is_ok(), "{answer:?}");
    let visited: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_string())
        .collect();
    assert_eq!(visited, ["/start", "/moved/first", "/moved/second"]);
    let landed = arrivals(&server, "/moved/second").await;
    assert_eq!(landed[0].body_json::<Value>().unwrap(), payload());
}

#[tokio::test]
async fn a_write_redirected_without_a_location_that_can_be_read_is_refused() {
    let server = MockServer::start().await;
    let answers = [
        ("/missing", ResponseTemplate::new(307)),
        ("/unparsable", redirecting(308, "http://[")),
        (
            "/unreadable",
            ResponseTemplate::new(307)
                .insert_header("location", HeaderValue::from_bytes(b"/to/\xff").unwrap()),
        ),
    ];
    for (route, answer) in answers.iter() {
        Mock::given(path(*route))
            .respond_with(answer.clone())
            .mount(&server)
            .await;
    }
    let service = stand_in(&server).await;

    for (route, _) in answers.iter() {
        let answer = writing(&service, Method::PUT, address(&server, route)).await;

        assert!(refused(&answer), "{route}: {answer:?}");
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        answers.len(),
        "nothing is sent on from a redirect whose Location cannot be read"
    );
}

#[tokio::test]
async fn a_write_sent_on_is_refused_when_it_is_redirected_again_by_a_status_that_may_change_it() {
    let server = MockServer::start().await;
    Mock::given(path("/first"))
        .respond_with(redirecting(307, format!("{}/second", server.uri())))
        .mount(&server)
        .await;
    Mock::given(path("/second"))
        .respond_with(redirecting(301, format!("{}/third", server.uri())))
        .mount(&server)
        .await;
    Mock::given(path("/third"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let answer = writing(&service, Method::PUT, address(&server, "/first")).await;

    assert!(refused(&answer), "{answer:?}");
    let second = arrivals(&server, "/second").await;
    assert_eq!(second.len(), 1, "the 307 is followed once");
    assert_eq!(second[0].body_json::<Value>().unwrap(), payload());
    assert!(
        arrivals(&server, "/third").await.is_empty(),
        "the 301 at the second hop is not followed"
    );
}

/// A body that is streamed can be sent only once, so a write that carries one
/// is refused where a redirect would have sent it again.
#[tokio::test]
async fn a_write_whose_body_cannot_be_sent_twice_is_refused_rather_than_sent_on() {
    let server = MockServer::start().await;
    Mock::given(path("/stream"))
        .respond_with(redirecting(307, format!("{}{TARGETS}", server.uri())))
        .mount(&server)
        .await;
    Mock::given(path(TARGETS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let answer = answered(
        service
            .request(Method::PUT, address(&server, "/stream"), &token(), ACCEPT)
            .body(Body::wrap(String::from("sent once"))),
    )
    .await;

    assert!(refused(&answer), "{answer:?}");
    assert_eq!(arrivals(&server, "/stream").await[0].body, b"sent once");
    assert!(
        arrivals(&server, TARGETS).await.is_empty(),
        "a body that cannot be sent again is not sent on"
    );
}

/// A write sent on keeps what is left of the time it was first given rather
/// than starting again at each hop, so redirects that each answer in time but
/// together take longer are given up on as one stalled request is, and the
/// write never reaches where they lead.
#[tokio::test]
async fn a_write_sent_on_is_given_up_on_once_its_hops_together_outlast_the_timeout() {
    let server = MockServer::start().await;
    for (from, to) in [
        ("/first", "/second"),
        ("/second", "/third"),
        ("/third", TARGETS),
    ] {
        Mock::given(path(from))
            .respond_with(
                redirecting(307, format!("{}{to}", server.uri()))
                    .set_delay(Duration::from_millis(120)),
            )
            .mount(&server)
            .await;
    }
    Mock::given(path(TARGETS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let service = PullRequestService::addressing(
        Origin::standing_in_for("github.com", &server.uri()).unwrap(),
        false,
        Duration::from_millis(200),
    )
    .unwrap();

    let answer = writing(&service, Method::PUT, address(&server, "/first")).await;

    assert!(
        matches!(answer, Err(PullRequestError::Http(ref error)) if error.is_timeout()),
        "{answer:?}"
    );
    assert!(
        arrivals(&server, TARGETS).await.is_empty(),
        "a write whose time ran out on the way is never sent on to where it was led"
    );
}
