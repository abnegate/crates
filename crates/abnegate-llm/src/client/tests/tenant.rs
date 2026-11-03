use std::process::Command;
use std::time::Duration;

use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::client::Trust;
use crate::client::pool::guarded;
use crate::client::reporter::ELLIPSIS;
use crate::client::reporter::REPORTED_LIMIT;
use crate::client::reporter::UNEXPLAINED;
use crate::error::Error;
use crate::wire::Message;

const KEY: &str = "sk-tenant-AbCdEfGh1234567890wxyz";

fn client(base_url: &str) -> LlmClient {
    LlmClient::new(LlmConfig::new(base_url, "gpt-4", KEY).with_trust(Trust::Tenant))
}

fn completion() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "completion",
        "object": "chat.completion",
        "created": 0,
        "model": "test",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "hi"},
            "finish_reason": "stop"
        }]
    }))
}

async fn refusal(response: ResponseTemplate) -> Error {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    client(&server.uri())
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("the endpoint refused the turn")
}

async fn streamed_failure(error: serde_json::Value) -> String {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {}\n\n", json!({ "error": error }))),
        )
        .mount(&server)
        .await;
    let failures: Vec<Error> = client(&server.uri())
        .chat_stream(&[Message::user("hi")], None)
        .await
        .expect("the stream opens")
        .filter_map(|result| async move { result.err() })
        .collect()
        .await;
    let [Error::Stream(message)] = failures.as_slice() else {
        panic!("{failures:?}");
    };
    message.clone()
}

fn reported(error: &Error) -> (u16, String) {
    let Error::Api { status, message } = error else {
        panic!("{error:?}");
    };
    let envelope: serde_json::Value = serde_json::from_str(message).expect("an error envelope");
    let text = envelope["error"]["message"]
        .as_str()
        .expect("an error message")
        .to_string();
    assert_eq!(
        envelope,
        json!({ "error": { "message": text } }),
        "{message}"
    );
    (*status, text)
}

#[tokio::test]
async fn a_tenant_endpoint_is_never_sent_to_a_metadata_address() {
    for base_url in [
        "http://169.254.169.254/latest",
        "http://169.254.170.2/v1",
        "http://[fe80::1]:8080/v1",
        "http://[fd00:ec2::254]/v1",
        "http://[::ffff:169.254.169.254]/v1",
        "http://[fd20:ce::254]/computeMetadata/v1",
        "http://168.63.129.16/machine",
        "http://metadata.google.internal/computeMetadata/v1",
        "http://METADATA.GOOGLE.INTERNAL./v1",
    ] {
        let refused = client(base_url).chat(&[Message::user("hi")], None).await;

        assert!(
            matches!(&refused, Err(Error::InvalidConfig(message)) if message.contains("metadata")),
            "{base_url}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_tenant_stream_is_never_sent_to_a_metadata_address() {
    let refused = client("http://169.254.169.254/latest")
        .chat_stream(&[Message::user("hi")], None)
        .await
        .err();

    assert!(
        matches!(&refused, Some(Error::InvalidConfig(message)) if message.contains("metadata")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_operator_endpoint_may_name_a_link_local_address() {
    let config = LlmConfig::new("http://169.254.169.254:1/v1", "gpt-4", KEY)
        .with_timeout(Duration::from_millis(200));

    let refused = LlmClient::new(config)
        .chat(&[Message::user("hi")], None)
        .await;

    assert!(
        !matches!(&refused, Err(Error::InvalidConfig(_))),
        "the operator's own endpoint was refused: {refused:?}"
    );
}

#[tokio::test]
async fn a_tenant_endpoint_is_never_redirected_to_a_metadata_address() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let refused = client(&server.uri())
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("a redirect is refused");

    assert_eq!(reported(&refused), (307, "Temporary Redirect".to_string()));
    server.verify().await;
}

#[tokio::test]
async fn a_tenant_endpoint_is_never_followed_through_a_redirect() {
    let elsewhere = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&elsewhere)
        .await;
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/admin", elsewhere.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;

    let refused = client(&server.uri())
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("a redirect is refused");

    assert_eq!(reported(&refused), (307, "Temporary Redirect".to_string()));
    server.verify().await;
    elsewhere.verify().await;
}

async fn proxied(trust: Trust) -> (usize, usize) {
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&proxy)
        .await;
    let endpoint = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&endpoint)
        .await;
    let builder = Client::builder().proxy(reqwest::Proxy::all(proxy.uri()).expect("a proxy URL"));
    let client = guarded(builder, trust).build().expect("a client");

    let sent = client.post(endpoint.uri()).send().await;

    assert!(sent.is_ok(), "{trust:?}: {sent:?}");
    (reached(&proxy).await, reached(&endpoint).await)
}

async fn reached(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

#[tokio::test]
async fn a_tenant_endpoint_is_never_sent_through_a_proxy() {
    assert_eq!(
        proxied(Trust::Operator).await,
        (1, 0),
        "the operator's client goes through the proxy it was given"
    );
    assert_eq!(
        proxied(Trust::Tenant).await,
        (0, 1),
        "a proxy resolves the tenant's host itself, past the metadata guard"
    );
}

#[tokio::test]
async fn a_tenant_endpoint_still_reaches_loopback() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;
    let localhost = server.uri().replace("127.0.0.1", "localhost");

    let answered = client(&localhost).chat(&[Message::user("hi")], None).await;

    assert!(answered.is_ok(), "{answered:?}");
    server.verify().await;
}

#[tokio::test]
async fn a_tenant_refusal_reports_only_its_status_and_error_message() {
    let error = refusal(ResponseTemplate::new(400).set_body_json(json!({
        "error": {
            "message": "The model `gpt-9` does not exist",
            "type": "invalid_request_error",
        },
        "reflected": "ami-id: i-0123456789abcdef0",
    })))
    .await;

    assert_eq!(
        reported(&error),
        (400, "The model `gpt-9` does not exist".to_string())
    );
    assert!(!error.to_string().contains("ami-id"), "{error}");
}

#[tokio::test]
async fn a_tenant_refusal_without_an_error_message_reports_its_status_alone() {
    for body in [
        "<html><body>instance-id: i-0123456789abcdef0</body></html>",
        r#"{"Code":"Success","AccessKeyId":"ASIAEXAMPLE"}"#,
        "",
    ] {
        let error = refusal(ResponseTemplate::new(502).set_body_string(body)).await;

        assert_eq!(reported(&error), (502, "Bad Gateway".to_string()), "{body}");
    }
}

#[tokio::test]
async fn a_tenant_refusal_is_capped_and_never_carries_the_key() {
    let long = format!("{}{KEY} sk-tenant-****wxyz", "x".repeat(2_000));
    let error = refusal(
        ResponseTemplate::new(401)
            .set_body_json(json!({ "error": { "message": format!("bad key {KEY}") } })),
    )
    .await;
    let (_, message) = reported(&error);
    assert!(!message.contains(KEY), "{message}");

    let error =
        refusal(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": long } })))
            .await;
    let (_, message) = reported(&error);

    assert_eq!(message.chars().count(), REPORTED_LIMIT + 1);
    assert!(message.ends_with(ELLIPSIS), "{message}");
    assert!(!message.contains("wxyz"), "{message}");
}

#[tokio::test]
async fn a_tenant_refusal_still_says_when_a_model_takes_no_tools() {
    for body in [
        json!({ "error": "llava:7b does not support tools" }),
        json!({ "error": { "message": "llava:7b does not support tools" } }),
    ] {
        let error = refusal(ResponseTemplate::new(400).set_body_json(body)).await;

        assert!(error.unsupported_tools(), "{error:?}");
    }
}

#[tokio::test]
async fn a_tenant_error_mid_stream_reports_only_its_own_message() {
    let message = streamed_failure(json!({ "message": format!("bad key {KEY}") })).await;
    assert!(message.starts_with("bad key"), "{message}");
    assert!(!message.contains(KEY), "{message}");

    let message = streamed_failure(json!("the model is overloaded")).await;
    assert_eq!(message, "the model is overloaded");
}

#[tokio::test]
async fn a_tenant_error_mid_stream_without_a_message_reports_nothing_it_sent() {
    for error in [
        json!({ "detail": "ami-id: i-0123456789abcdef0" }),
        json!({ "message": { "ami-id": "i-0123456789abcdef0" } }),
        json!(["ami-id", "i-0123456789abcdef0"]),
    ] {
        let message = streamed_failure(error.clone()).await;

        assert_eq!(message, UNEXPLAINED, "{error}");
    }
}

#[tokio::test]
async fn a_tenant_error_mid_stream_is_capped() {
    let message = streamed_failure(json!({ "message": "x ".repeat(2_000) })).await;

    assert_eq!(message.chars().count(), REPORTED_LIMIT + 1);
    assert!(message.ends_with(ELLIPSIS), "{message}");
}

#[tokio::test]
async fn a_blank_message_gives_way_to_the_next_one_a_refusal_carries() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {"message": "  "},
            "message": "model unavailable",
        })))
        .mount(&server)
        .await;

    let error = client(&format!("{}/v1", server.uri()))
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("refused");

    match &error {
        Error::Api { status, message } => {
            assert_eq!(*status, 503);
            assert!(message.contains("model unavailable"), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A tenant endpoint that refuses with a body it never finishes sending.
fn endless_refusal() -> (String, std::thread::JoinHandle<()>) {
    use std::io::BufRead;
    use std::io::Read;
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bound");
    let address = listener.local_addr().expect("address");
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accepted");
        let mut writer = stream.try_clone().expect("cloned");
        let mut reader = std::io::BufReader::new(stream);
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

        let chunk = "x".repeat(16 * 1024);
        let _ = writer.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
        );
        for _ in 0..16 {
            let _ = writer.write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes());
        }
        let _ = writer.flush();
        std::thread::sleep(ENDLESS_FOR);
    });
    (format!("http://{address}/v1"), handle)
}

const ENDLESS_FOR: Duration = Duration::from_secs(20);

#[tokio::test]
async fn a_tenant_refusal_is_read_no_further_than_its_cap() {
    let (endpoint, server) = endless_refusal();
    let started = std::time::Instant::now();

    let error = client(&endpoint)
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("refused");

    assert!(
        started.elapsed() < ENDLESS_FOR / 2,
        "waited {:?} for the rest of an endless body",
        started.elapsed()
    );
    match &error {
        Error::Api { status, message } => {
            assert_eq!(*status, 400);
            assert!(message.contains("Bad Request"), "{message}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    drop(server);
}

const CHILD: &str = "ABNEGATE_LLM_TEST_CHILD";
const PROXY: &str = "ABNEGATE_LLM_TEST_PROXY";
const ENDPOINT: &str = "ABNEGATE_LLM_TEST_ENDPOINT";
const PROXY_VARIABLES: [&str; 4] = ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];
const PROXY_EXCLUSIONS: [&str; 3] = ["NO_PROXY", "no_proxy", "REQUEST_METHOD"];

fn answering(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "completion",
        "object": "chat.completion",
        "created": 0,
        "model": "test",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }]
    }))
}

async fn answered_by(trust: Trust, endpoint: &str) -> String {
    let config = LlmConfig::new(endpoint, "gpt-4", KEY).with_trust(trust);
    let response = LlmClient::new(config)
        .chat(&[Message::user("hi")], None)
        .await
        .unwrap_or_else(|error| panic!("{trust:?}: {error}"));
    response.choices[0]
        .message
        .content
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// The pooled clients read the proxy environment once, when they are built,
/// so the environment is set on a child process that runs this test again.
#[tokio::test]
async fn a_pooled_tenant_client_never_goes_through_the_proxy_environment() {
    const NAME: &str =
        "client::tests::tenant::a_pooled_tenant_client_never_goes_through_the_proxy_environment";
    if std::env::var(CHILD).as_deref() == Ok(NAME) {
        let endpoint = std::env::var(ENDPOINT).expect("the endpoint");
        assert_eq!(answered_by(Trust::Operator, &endpoint).await, PROXY);
        assert_eq!(answered_by(Trust::Tenant, &endpoint).await, ENDPOINT);
        return;
    }

    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(answering(PROXY))
        .expect(1)
        .mount(&proxy)
        .await;
    let endpoint = MockServer::start().await;
    Mock::given(any())
        .respond_with(answering(ENDPOINT))
        .expect(1)
        .mount(&endpoint)
        .await;
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args(["--exact", NAME, "--nocapture"])
        .env(CHILD, NAME)
        .env(ENDPOINT, format!("{}/v1", endpoint.uri()));
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
