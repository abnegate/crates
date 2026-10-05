use abnegate_secret::REDACTED;
use futures::StreamExt;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::error::Error;
use crate::wire::Message;

const KEY: &str = "sk-proj-AbCdEfGh1234567890wxyz";

fn echoes() -> Vec<String> {
    vec![
        format!(r#"{{"error":{{"message":"bad key {KEY}"}}}}"#),
        r#"{"error":{"message":"Incorrect API key provided: sk-proj-****************wxyz."}}"#
            .to_string(),
        r#"{"error":"{\"message\":\"invalid x-api-key: ****wxyz\"}"}"#.to_string(),
        r#"{"detail":"litellm.AuthenticationError: api_key=sk-proj-AbCd****"}"#.to_string(),
        "rejected key:sk-proj-AbCd****".to_string(),
    ]
}

async fn refused(body: &str, stream: bool) -> Error {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401).set_body_string(body))
        .mount(&server)
        .await;
    let client = LlmClient::new(LlmConfig::new(server.uri(), "gpt-4", KEY));
    let messages = [Message::user("hi")];
    if stream {
        client
            .chat_stream(&messages, None)
            .await
            .err()
            .expect("the endpoint refused the stream")
    } else {
        client
            .chat(&messages, None)
            .await
            .expect_err("the endpoint refused the turn")
    }
}

#[tokio::test]
async fn a_refusal_never_carries_the_key_it_was_sent() {
    for body in echoes() {
        for stream in [false, true] {
            let error = refused(&body, stream).await;
            let Error::Api { status, message } = &error else {
                panic!("{error:?}");
            };
            let shown = error.to_string();

            assert_eq!(*status, 401);
            for leaked in [KEY, "wxyz", "sk-proj-AbCd"] {
                assert!(!message.contains(leaked), "{body} -> {message}");
                assert!(!shown.contains(leaked), "{body} -> {shown}");
            }
            assert!(message.contains(REDACTED), "{message}");
        }
    }
}

#[tokio::test]
async fn a_refusal_without_the_key_is_reported_as_the_endpoint_sent_it() {
    let body = r#"{"error":{"message":"The model `gpt-9` does not exist"}}"#;

    let error = refused(body, false).await;

    assert!(
        matches!(&error, Error::Api { status: 401, message } if message == body),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_error_sent_mid_stream_never_carries_the_key_it_was_sent() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "data: {\"error\":{\"message\":",
                    "\"Incorrect API key provided: sk-proj-****wxyz\"}}\n\n"
                )),
        )
        .mount(&server)
        .await;

    let failures: Vec<Error> = LlmClient::new(LlmConfig::new(server.uri(), "gpt-4", KEY))
        .chat_stream(&[Message::user("hi")], None)
        .await
        .expect("the stream opens")
        .filter_map(|result| async move { result.err() })
        .collect()
        .await;

    let [Error::Stream(message)] = failures.as_slice() else {
        panic!("{failures:?}");
    };
    assert!(
        message.starts_with("Incorrect API key provided"),
        "{message}"
    );
    for leaked in [KEY, "wxyz", "sk-proj-"] {
        assert!(!message.contains(leaked), "{message}");
    }
}
