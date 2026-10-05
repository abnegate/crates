use serde_json::json;
use wiremock::Match;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::error::Error;
use crate::wire::ChatResponse;
use crate::wire::Message;

struct Unauthorized;

impl Match for Unauthorized {
    fn matches(&self, request: &Request) -> bool {
        !request.headers.contains_key(reqwest::header::AUTHORIZATION)
    }
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

async fn turn(server: &MockServer, api_key: &str) -> Result<ChatResponse, Error> {
    LlmClient::new(LlmConfig::new(server.uri(), "test", api_key))
        .chat(&[Message::user("hi")], None)
        .await
}

#[tokio::test]
async fn an_endpoint_without_a_key_is_sent_no_authorization_header() {
    for blank in ["", "   ", "\t\n"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(Unauthorized)
            .respond_with(completion())
            .expect(1)
            .mount(&server)
            .await;

        let answered = turn(&server, blank).await;

        assert!(answered.is_ok(), "{blank:?}: {answered:?}");
        server.verify().await;
    }
}

#[tokio::test]
async fn an_endpoint_with_a_key_is_sent_it_trimmed_as_a_bearer_token() {
    for key in ["sk-endpoint-key", "  sk-endpoint-key\n"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-endpoint-key"))
            .respond_with(completion())
            .expect(1)
            .mount(&server)
            .await;

        let answered = turn(&server, key).await;

        assert!(answered.is_ok(), "{key:?}: {answered:?}");
        server.verify().await;
    }
}
