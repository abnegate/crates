//! What a third-party notifier can do with an `Endpoint` and a `Webhook`.

use std::collections::BTreeMap;
use std::time::Duration;

use abnegate_notify::Endpoint;
use abnegate_notify::Error;
use abnegate_notify::Webhook;
use serde_json::json;

const ALLOWED: &[&str] = &["hooks.example.invalid"];
const SECRET: &str = "xxxxSECRETxxxx";

fn endpoint() -> Endpoint {
    Endpoint::new(
        &format!("https://HOOKS.Example.INVALID/services/T000/{SECRET}"),
        ALLOWED,
    )
    .expect("allowed host")
}

#[test]
fn an_endpoint_reads_back_the_url_it_checked() {
    let endpoint = endpoint();

    assert_eq!(
        endpoint.url().expose(),
        format!("https://hooks.example.invalid/services/T000/{SECRET}")
    );
    assert_eq!(endpoint.host(), "hooks.example.invalid");
}

#[test]
fn the_read_back_url_stays_redacted_when_formatted() {
    let endpoint = endpoint();

    assert!(!format!("{:?}", endpoint.url()).contains(SECRET));
    assert!(!endpoint.url().to_string().contains(SECRET));
}

#[test]
fn a_webhook_reports_its_endpoint_host_and_budget() {
    let webhook = Webhook::new(endpoint()).expect("client");
    assert_eq!(webhook.host(), "hooks.example.invalid");
    assert_eq!(webhook.endpoint().host(), "hooks.example.invalid");
    assert_eq!(webhook.timeout(), None);

    let bounded = webhook.with_timeout(Duration::from_secs(2));
    assert_eq!(bounded.timeout(), Some(Duration::from_secs(2)));
}

#[test]
fn a_zero_budget_is_raised_above_zero() {
    let webhook = Webhook::new(endpoint())
        .expect("client")
        .with_timeout(Duration::ZERO);
    assert!(
        webhook
            .timeout()
            .is_some_and(|timeout| timeout > Duration::ZERO)
    );
}

#[tokio::test]
async fn a_failed_post_names_the_host_and_never_the_url() {
    let webhook = Webhook::new(endpoint())
        .expect("client")
        .with_timeout(Duration::from_secs(5));

    let error = webhook
        .post(&json!({ "text": "hello" }))
        .await
        .expect_err("an .invalid host never resolves");

    assert!(
        matches!(error, Error::Unreachable { .. } | Error::Timeout { .. }),
        "unexpected {error:?}"
    );
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains(SECRET), "leaked: {rendered}");
    assert!(!rendered.contains("/services/"), "leaked: {rendered}");
}

/// A payload that cannot be serialized was reported by the transport as
/// unreachable, and so as worth retrying, when sending it again unchanged can
/// never succeed.
#[tokio::test]
async fn a_payload_that_cannot_be_serialized_is_malformed_and_never_retried() {
    let webhook = Webhook::new(endpoint()).expect("client");
    let payload = BTreeMap::from([((1_u8, 2_u8), "a key JSON cannot hold")]);

    let error = webhook
        .post(&payload)
        .await
        .expect_err("a tuple key has no JSON form");

    assert!(
        matches!(error, Error::Malformed { .. }),
        "unexpected {error:?}"
    );
    assert!(!error.is_retryable());
}

#[cfg(feature = "testing")]
mod served {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    use abnegate_notify::Endpoint;
    use abnegate_notify::EndpointError;
    use abnegate_notify::Error;
    use abnegate_notify::Webhook;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::Deserialize;
    use serde::Serialize;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::basic_auth;
    use wiremock::matchers::bearer_token;
    use wiremock::matchers::body_string;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    const TOKEN: &str = "letmein-bearer-77";
    const PASSWORD: &str = "letmein-basic-88";
    const URL_SECRET: &str = "xxxxSECRETxxxx";
    const FORM: &str = "application/x-www-form-urlencoded";

    #[derive(Debug, Deserialize, PartialEq)]
    struct Created {
        id: String,
        channel_id: String,
    }

    #[derive(Serialize)]
    struct Text<'a> {
        #[serde(rename = "To")]
        to: &'a str,
        #[serde(rename = "Body")]
        body: &'a str,
    }

    fn webhook(server: &MockServer) -> Webhook {
        Webhook::for_test(&format!("{}/hooks/{URL_SECRET}", server.uri())).expect("loopback")
    }

    fn assert_concealed(error: &Error) {
        let rendered = format!("{error} {error:?}");
        for secret in [TOKEN, PASSWORD, URL_SECRET] {
            assert!(!rendered.contains(secret), "{secret} leaked: {rendered}");
        }
    }

    #[tokio::test]
    async fn a_bearer_token_rides_in_the_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(bearer_token(TOKEN))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        webhook(&server)
            .with_bearer(TOKEN)
            .post(&json!({ "text": "hi" }))
            .await
            .expect("delivered");
    }

    #[tokio::test]
    async fn basic_credentials_ride_in_the_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(basic_auth("AC000", PASSWORD))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        webhook(&server)
            .with_basic("AC000", PASSWORD)
            .post(&json!({ "text": "hi" }))
            .await
            .expect("delivered");
    }

    #[tokio::test]
    async fn the_last_credential_set_is_the_one_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(basic_auth("AC000", PASSWORD))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        webhook(&server)
            .with_bearer(TOKEN)
            .with_basic("AC000", PASSWORD)
            .post(&json!({}))
            .await
            .expect("delivered with the basic credential");
    }

    #[tokio::test]
    async fn a_form_is_sent_url_encoded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("content-type", FORM))
            .and(body_string(
                "To=%2B15550001111&Body=deploy+finished+%26+green",
            ))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;

        webhook(&server)
            .post_form(&Text {
                to: "+15550001111",
                body: "deploy finished & green",
            })
            .await
            .expect("delivered");
    }

    #[tokio::test]
    async fn a_form_with_basic_credentials_reads_back_what_was_created() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(basic_auth("AC000", PASSWORD))
            .and(header("content-type", FORM))
            .and(body_string("To=%2B1&Body=hi"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "id": "SM1", "channel_id": "sms", "extra": true })),
            )
            .mount(&server)
            .await;

        let created: Created = webhook(&server)
            .with_basic("AC000", PASSWORD)
            .post_form_for(&[("To", "+1"), ("Body", "hi")])
            .await
            .expect("parsed");

        assert_eq!(
            created,
            Created {
                id: "SM1".to_string(),
                channel_id: "sms".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_form_that_cannot_be_encoded_is_malformed_and_sends_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let error = webhook(&server)
            .post_form(&json!({ "nested": { "deeper": 1 } }))
            .await
            .expect_err("a nested map has no form encoding");

        assert!(
            matches!(error, Error::Malformed { .. }),
            "unexpected {error:?}"
        );
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn the_answer_is_parsed_into_the_callers_type() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/hooks/{URL_SECRET}")))
            .and(query_param("wait", "true"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "1001", "channel_id": "2002" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let webhook = Webhook::for_test(&format!("{}/hooks/{URL_SECRET}?wait=true", server.uri()))
            .expect("loopback");
        let created: Created = webhook
            .post_for(&json!({ "content": "hi" }))
            .await
            .expect("parsed");

        assert_eq!(created.id, "1001");
        assert_eq!(created.channel_id, "2002");
    }

    #[tokio::test]
    async fn the_raw_answer_is_returned_as_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok: queued"))
            .mount(&server)
            .await;

        let body = webhook(&server)
            .post_for_body(&json!({}))
            .await
            .expect("read");
        assert_eq!(body, b"ok: queued");
    }

    #[tokio::test]
    async fn an_answer_of_the_wrong_shape_is_unreadable_and_never_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "id": 7, "echo": TOKEN })),
            )
            .mount(&server)
            .await;

        let error = webhook(&server)
            .with_bearer(TOKEN)
            .post_for::<Created, _>(&json!({}))
            .await
            .expect_err("the id is not a string");

        match &error {
            Error::Unreadable { host, .. } => assert_eq!(host, "127.0.0.1"),
            other => panic!("expected an unreadable answer, got {other:?}"),
        }
        assert!(!error.is_retryable());
        assert_concealed(&error);
    }

    #[tokio::test]
    async fn an_answer_past_the_body_limit_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4_096)))
            .mount(&server)
            .await;

        let webhook = webhook(&server).with_body_limit(1_024);
        assert_eq!(webhook.body_limit(), 1_024);

        let error = webhook
            .post_for_body(&json!({}))
            .await
            .expect_err("larger than the limit");
        assert!(
            matches!(error, Error::Unreadable { .. }),
            "unexpected {error:?}"
        );
        assert!(error.to_string().contains("1024 byte limit"), "{error}");
    }

    /// A server on loopback that answers one request with `size` bytes in
    /// chunks, so no `Content-Length` announces how much is coming.
    fn chunked_server(size: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepted");
            let mut writer = stream.try_clone().expect("cloned");
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read");
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().expect("a length");
                }
            }
            let mut body = vec![0_u8; length];
            reader.read_exact(&mut body).expect("the request body");

            let chunk = "x".repeat(512);
            let mut answer = String::from(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            );
            for _ in 0..size / chunk.len() {
                answer.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
            }
            answer.push_str("0\r\n\r\n");
            let _ = writer.write_all(answer.as_bytes());
        });
        (format!("http://{address}/hook"), handle)
    }

    #[tokio::test]
    async fn a_chunked_answer_past_the_body_limit_is_refused() {
        let (url, server) = chunked_server(4_096);

        let error = Webhook::for_test(&url)
            .expect("loopback")
            .with_body_limit(1_024)
            .post_for_body(&json!({}))
            .await
            .expect_err("larger than the limit");
        assert!(
            matches!(error, Error::Unreadable { .. }),
            "unexpected {error:?}"
        );
        assert!(error.to_string().contains("1024 byte limit"), "{error}");
        server.join().expect("served");
    }

    #[tokio::test]
    async fn a_basic_user_name_holding_a_colon_is_refused_before_anything_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let error = webhook(&server)
            .with_basic("AC:000", PASSWORD)
            .post(&json!({}))
            .await
            .expect_err("a colon cannot travel in a basic user name");
        assert!(
            matches!(error, Error::Malformed { .. }),
            "unexpected {error:?}"
        );
        assert!(!error.is_retryable());
        assert_concealed(&error);
    }

    #[tokio::test]
    async fn an_echoed_credential_in_the_url_path_is_concealed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string(format!("invalid webhook {URL_SECRET}")),
            )
            .mount(&server)
            .await;

        let error = webhook(&server)
            .post(&json!({}))
            .await
            .expect_err("not found");
        match &error {
            Error::Rejected { body, .. } => assert!(body.contains("invalid webhook"), "{body}"),
            other => panic!("expected a rejection, got {other:?}"),
        }
        assert_concealed(&error);
    }

    /// The client reads the proxy environment once, when it is built, so
    /// the environment is set on a child process that runs this test again.
    #[tokio::test]
    async fn a_test_client_never_goes_through_the_proxy_environment() {
        const NAME: &str = "served::a_test_client_never_goes_through_the_proxy_environment";
        const CHILD: &str = "ABNEGATE_NOTIFY_TEST_CHILD";
        const ENDPOINT: &str = "ABNEGATE_NOTIFY_TEST_ENDPOINT";
        const PROXY_VARIABLES: [&str; 4] = ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];
        const PROXY_EXCLUSIONS: [&str; 3] = ["NO_PROXY", "no_proxy", "REQUEST_METHOD"];

        if std::env::var(CHILD).as_deref() == Ok(NAME) {
            let endpoint = std::env::var(ENDPOINT).expect("the endpoint");
            Webhook::for_test(&endpoint)
                .expect("loopback")
                .with_bearer(TOKEN)
                .post(&json!({}))
                .await
                .expect("delivered straight to the endpoint");
            return;
        }

        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502))
            .expect(0)
            .mount(&proxy)
            .await;
        let endpoint = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&endpoint)
            .await;
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, NAME)
            .env(ENDPOINT, format!("{}/hook", endpoint.uri()));
        for variable in PROXY_VARIABLES {
            command.env(variable, proxy.uri());
        }
        for variable in PROXY_EXCLUSIONS {
            command.env_remove(variable);
        }

        let output = tokio::task::spawn_blocking(move || command.output())
            .await
            .expect("the child is waited for")
            .expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        proxy.verify().await;
        endpoint.verify().await;
    }

    #[test]
    fn the_body_limit_starts_at_the_default() {
        let webhook = Webhook::for_test("http://127.0.0.1:1/hook").expect("loopback");
        assert_eq!(webhook.body_limit(), Webhook::DEFAULT_BODY_LIMIT);
    }

    #[tokio::test]
    async fn a_rate_limit_is_reported_for_every_kind_of_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
            .mount(&server)
            .await;
        let webhook = webhook(&server).with_bearer(TOKEN);

        let errors = [
            webhook
                .post_for::<Created, _>(&json!({}))
                .await
                .expect_err("rate limited"),
            webhook
                .post_for_body(&json!({}))
                .await
                .expect_err("rate limited"),
            webhook
                .post_form(&[("a", "b")])
                .await
                .expect_err("rate limited"),
            webhook
                .post_form_for::<Created, _>(&[("a", "b")])
                .await
                .expect_err("rate limited"),
        ];

        for error in errors {
            assert!(
                matches!(error, Error::RateLimited { .. }),
                "unexpected {error:?}"
            );
            assert_eq!(error.retry_after(), Some(Duration::from_secs(2)));
        }
    }

    #[tokio::test]
    async fn a_redirect_is_refused_and_its_target_never_hears_of_it() {
        let target = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&target)
            .await;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", format!("{}/", target.uri())),
            )
            .mount(&server)
            .await;
        let webhook = webhook(&server).with_bearer(TOKEN);

        let errors = [
            webhook
                .post_for::<Created, _>(&json!({}))
                .await
                .expect_err("a redirect is not a delivery"),
            webhook
                .post_form(&[("a", "b")])
                .await
                .expect_err("a redirect is not a delivery"),
        ];

        for error in errors {
            match error {
                Error::Rejected { status, .. } => assert_eq!(status, 307),
                other => panic!("expected the redirect to be reported, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_credential_echoed_in_a_rejection_is_concealed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(bearer_token(TOKEN))
            .respond_with(
                ResponseTemplate::new(401).set_body_string(format!("nope: {TOKEN} is revoked")),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(basic_auth("AC000", PASSWORD))
            .respond_with(
                ResponseTemplate::new(401).set_body_string(format!("nope: {PASSWORD} is revoked")),
            )
            .mount(&server)
            .await;

        let bearer = webhook(&server)
            .with_bearer(TOKEN)
            .post(&json!({}))
            .await
            .expect_err("unauthorized");
        let basic = webhook(&server)
            .with_basic("AC000", PASSWORD)
            .post_form(&[("a", "b")])
            .await
            .expect_err("unauthorized");

        for error in [bearer, basic] {
            match &error {
                Error::Rejected { status, body, .. } => {
                    assert_eq!(*status, 401);
                    assert!(body.contains("is revoked"), "{body}");
                }
                other => panic!("expected a rejection, got {other:?}"),
            }
            assert_concealed(&error);
        }
    }

    #[tokio::test]
    async fn an_echoed_basic_header_is_concealed() {
        let encoded = STANDARD.encode(format!("AC000:{PASSWORD}"));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string(format!(
                "unauthorized request: Authorization: Basic {encoded} (credential {encoded}) was refused"
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/answer"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(encoded)))
            .with_priority(1)
            .mount(&server)
            .await;

        let rejected = webhook(&server)
            .with_basic("AC000", PASSWORD)
            .post_form(&[("a", "b")])
            .await
            .expect_err("unauthorized");
        let unreadable = Webhook::for_test(&format!("{}/answer", server.uri()))
            .expect("loopback")
            .with_basic("AC000", PASSWORD)
            .post_for::<u64, _>(&json!({}))
            .await
            .expect_err("a string is not a number");

        assert!(
            matches!(rejected, Error::Rejected { status: 401, .. }),
            "unexpected {rejected:?}"
        );
        assert!(
            matches!(unreadable, Error::Unreadable { .. }),
            "unexpected {unreadable:?}"
        );
        for error in [rejected, unreadable] {
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.contains(&encoded), "encoded leaked: {rendered}");
            assert_concealed(&error);
        }
    }

    #[tokio::test]
    async fn an_unsendable_credential_is_malformed_and_never_quoted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let error = webhook(&server)
            .with_bearer(format!("{TOKEN}\r\nX-Injected: yes"))
            .post(&json!({}))
            .await
            .expect_err("a header cannot carry a line break");

        assert!(
            matches!(error, Error::Malformed { .. }),
            "unexpected {error:?}"
        );
        assert!(!error.is_retryable());
        assert_concealed(&error);
    }

    #[tokio::test]
    async fn an_unreachable_server_never_names_the_url_or_the_credential() {
        let webhook = Webhook::for_test(&format!("http://127.0.0.1:1/hooks/{URL_SECRET}"))
            .expect("loopback")
            .with_basic("AC000", PASSWORD);

        let error = webhook
            .post_for_body(&json!({}))
            .await
            .expect_err("nothing listens on port 1");

        assert!(
            matches!(error, Error::Unreachable { .. }),
            "unexpected {error:?}"
        );
        assert_concealed(&error);
    }

    #[test]
    fn credentials_never_appear_in_debug() {
        for webhook in [
            Webhook::for_test(&format!("http://127.0.0.1:1/hooks/{URL_SECRET}"))
                .expect("loopback")
                .with_bearer(TOKEN),
            Webhook::for_test(&format!("http://127.0.0.1:1/hooks/{URL_SECRET}"))
                .expect("loopback")
                .with_basic("AC000", PASSWORD),
        ] {
            let debug = format!("{webhook:?}");
            for secret in [TOKEN, PASSWORD, URL_SECRET, "AC000"] {
                assert!(!debug.contains(secret), "{secret} leaked: {debug}");
            }
            assert!(debug.contains("[REDACTED]"), "{debug}");
        }
    }

    #[test]
    fn the_test_constructor_reaches_only_this_machine() {
        let error = Webhook::for_test("http://hooks.example.com/hook").expect_err("not loopback");
        assert!(
            matches!(
                &error,
                Error::Endpoint(EndpointError::HostNotAllowed { host, .. }) if host == "hooks.example.com"
            ),
            "unexpected {error:?}"
        );
        assert!(Endpoint::for_test("https://hooks.example.com/hook").is_err());
    }
}
