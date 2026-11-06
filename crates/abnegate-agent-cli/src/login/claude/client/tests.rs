use chrono::TimeDelta;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;
use crate::login::claude::CLIENT_ID;
use crate::login::claude::LIFETIME;
use crate::login::claude::REDIRECT_URL;
use crate::login::claude::Refusal;
use crate::test_support::delegated;

const TOKEN_PATH: &str = "/v1/oauth/token";
const CODE: &str = "fake-authorization-code";
const STATE: &str = "fake-state";
const VERIFIER: &str = "fake-code-verifier";
const ACCESS: &str = "fake-access-token";
const REFRESH: &str = "fake-refresh-token";
const GRANTED: u64 = 28_800;

fn client(server: &MockServer) -> Client {
    let endpoints = Endpoints::default()
        .with_token(&format!("{}{TOKEN_PATH}", server.uri()))
        .expect("a loopback token endpoint");
    Client::new(endpoints).expect("the client builds")
}

async fn answering(status: u16, body: Value) -> (MockServer, Client) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .and(header("content-type", "application/json"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    (server, client)
}

async fn sent(server: &MockServer) -> Value {
    let requests = server
        .received_requests()
        .await
        .expect("the server records requests");
    assert_eq!(requests.len(), 1, "one request per call");
    requests[0].body_json().expect("a JSON body")
}

fn code() -> Code {
    Code::new(CODE, STATE).expect("a code")
}

fn verifier() -> SecretValue {
    SecretValue::new(VERIFIER)
}

fn earlier() -> Tokens {
    Tokens::new(
        SecretValue::new(ACCESS),
        Some(SecretValue::new(REFRESH)),
        Utc::now(),
        None,
        "user:profile user:inference",
        Some("max".to_string()),
    )
}

fn seconds(seconds: u64) -> TimeDelta {
    TimeDelta::seconds(i64::try_from(seconds).expect("a small number of seconds"))
}

#[tokio::test]
async fn an_inference_exchange_asks_for_a_year_long_token() {
    let (server, client) = answering(
        200,
        json!({
            "access_token": ACCESS,
            "refresh_token": REFRESH,
            "expires_in": LIFETIME.as_secs(),
            "scope": "user:inference",
            "token_type": "Bearer",
        }),
    )
    .await;
    let before = Utc::now();

    let tokens = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect("exchange");

    assert_eq!(
        sent(&server).await,
        json!({
            "grant_type": "authorization_code",
            "code": CODE,
            "redirect_uri": REDIRECT_URL,
            "client_id": CLIENT_ID,
            "code_verifier": VERIFIER,
            "state": STATE,
            "expires_in": 31_536_000,
        })
    );
    assert_eq!(tokens.access.expose(), ACCESS);
    assert_eq!(
        tokens.refresh.as_ref().map(SecretValue::expose),
        Some(REFRESH)
    );
    assert_eq!(tokens.scope, "user:inference");
    let lifetime = seconds(LIFETIME.as_secs());
    assert!(
        tokens.expires_at >= before + lifetime && tokens.expires_at <= Utc::now() + lifetime,
        "{}",
        tokens.expires_at
    );
    assert_eq!(tokens.lifetime(), Some(LIFETIME));
}

#[tokio::test]
async fn a_full_exchange_leaves_the_lifetime_to_claude() {
    let (server, client) =
        answering(200, json!({"access_token": ACCESS, "expires_in": GRANTED})).await;

    let tokens = client
        .exchange(&code(), &verifier(), Scope::Full, Redirect::Paste)
        .await
        .expect("exchange");

    let body = sent(&server).await;
    assert!(
        body.get("expires_in").is_none(),
        "a full-access exchange must not ask for a lifetime: {body}"
    );
    assert_eq!(
        body,
        json!({
            "grant_type": "authorization_code",
            "code": CODE,
            "redirect_uri": REDIRECT_URL,
            "client_id": CLIENT_ID,
            "code_verifier": VERIFIER,
            "state": STATE,
        })
    );
    assert_eq!(
        tokens.scope,
        Scope::Full.parameter(),
        "a grant that names no scope keeps the scope asked for"
    );
    assert!(tokens.refresh.is_none());
    assert_eq!(tokens.lifetime(), Some(Duration::from_secs(GRANTED)));
}

#[tokio::test]
async fn a_loopback_exchange_sends_the_redirect_its_authorize_link_carried() {
    let (server, client) =
        answering(200, json!({"access_token": ACCESS, "expires_in": GRANTED})).await;

    client
        .exchange(
            &code(),
            &verifier(),
            Scope::Inference,
            Redirect::Loopback(54_545),
        )
        .await
        .expect("exchange");

    let body = sent(&server).await;
    assert_eq!(
        body["redirect_uri"], "http://localhost:54545/callback",
        "Claude grants a code only to the redirect it was issued for: {body}"
    );
    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], CODE);
}

#[tokio::test]
async fn a_refresh_sends_the_scope_and_keeps_a_refresh_token_it_was_not_sent_again() {
    let (server, client) = answering(
        200,
        json!({"access_token": "fake-renewed-access-token", "expires_in": GRANTED}),
    )
    .await;
    let earlier = earlier();

    let renewed = client.refresh(&earlier).await.expect("refresh");

    assert_eq!(
        sent(&server).await,
        json!({
            "grant_type": "refresh_token",
            "refresh_token": REFRESH,
            "client_id": CLIENT_ID,
            "scope": "user:profile user:inference",
        })
    );
    assert_eq!(renewed.access.expose(), "fake-renewed-access-token");
    assert_eq!(
        renewed.refresh, earlier.refresh,
        "Claude sent no new refresh token, so the old one stays"
    );
    assert_eq!(renewed.scope, earlier.scope);
    assert_eq!(renewed.subscription, earlier.subscription);
    assert_eq!(
        renewed.lifetime(),
        Some(Duration::from_secs(GRANTED)),
        "a renewal keeps the lifetime Claude granted it"
    );
}

#[tokio::test]
async fn a_refresh_of_tokens_that_name_no_scope_asks_for_the_one_granted() {
    let (server, client) = answering(
        200,
        json!({"access_token": "fake-renewed-access-token", "expires_in": GRANTED}),
    )
    .await;
    let tokens = Tokens {
        scope: String::new(),
        ..earlier()
    };

    client.refresh(&tokens).await.expect("refresh");

    assert_eq!(
        sent(&server).await,
        json!({
            "grant_type": "refresh_token",
            "refresh_token": REFRESH,
            "client_id": CLIENT_ID,
        }),
        "an empty scope asks for none at all; no scope keeps the one granted"
    );
}

#[tokio::test]
async fn a_refresh_that_rotates_the_refresh_token_keeps_the_new_one() {
    let (_server, client) = answering(
        200,
        json!({
            "access_token": "fake-renewed-access-token",
            "refresh_token": "fake-rotated-refresh-token",
            "expires_in": GRANTED,
            "subscription_type": "pro",
        }),
    )
    .await;

    let renewed = client.refresh(&earlier()).await.expect("refresh");

    assert_eq!(
        renewed.refresh.as_ref().map(SecretValue::expose),
        Some("fake-rotated-refresh-token")
    );
    assert_eq!(renewed.subscription.as_deref(), Some("pro"));
}

#[tokio::test]
async fn a_refresh_without_a_refresh_token_is_refused_before_any_request() {
    let server = MockServer::start().await;
    let client = client(&server);
    let tokens = Tokens {
        refresh: None,
        ..earlier()
    };

    let error = client.refresh(&tokens).await.expect_err("nothing to renew");

    assert!(
        matches!(error, LoginError::Malformed(UNRENEWABLE)),
        "{error:?}"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .map(|requests| requests.len()),
        Some(0)
    );
}

#[tokio::test]
async fn a_rejection_names_its_status_and_refusal_and_nothing_it_was_sent() {
    let description = format!("Code {CODE} with verifier {VERIFIER} does not answer state {STATE}");
    let (_server, client) = answering(
        400,
        json!({"error": "invalid_grant", "error_description": description}),
    )
    .await;

    let error = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("rejected");

    assert!(
        matches!(
            error,
            LoginError::Rejected {
                status: 400,
                refusal: Some(Refusal::Declined),
                ..
            }
        ),
        "{error:?}"
    );
    let shown = format!("{error} {error:?}");
    for sent in [CODE, VERIFIER, STATE] {
        assert!(!shown.contains(sent), "the refusal repeats {sent}: {shown}");
    }
}

#[tokio::test]
async fn a_rejected_body_never_reaches_the_error() {
    let body = "This sign-in was revoked for sk-ant-oat01-fake0123456789abcdefghij";
    for (status, answer, refusal) in [
        (
            401,
            json!({"type": "error", "error": {"type": "authentication_error", "message": body}}),
            None,
        ),
        (
            400,
            json!({"error": "invalid_grant", "error_description": "account_on_hold", "detail": body}),
            Some(Refusal::OnHold),
        ),
        (
            503,
            json!({"error": "temporarily_unavailable", "error_description": body}),
            Some(Refusal::Unavailable),
        ),
        (500, json!(body), None),
    ] {
        let (_server, client) = answering(status, answer).await;

        let error = client.refresh(&earlier()).await.expect_err("rejected");

        let LoginError::Rejected {
            status: rejected,
            refusal: named,
            ..
        } = &error
        else {
            panic!("expected a rejection, got {error:?}");
        };
        assert_eq!(*rejected, status);
        assert_eq!(*named, refusal, "{status}");
        let shown = format!("{error} {error:?}");
        for part in ["revoked", "sk-ant", "account_on_hold", REFRESH] {
            assert!(!shown.contains(part), "the error repeats {part}: {shown}");
        }
    }
}

#[tokio::test]
async fn an_unreadable_grant_is_refused_without_echoing_it() {
    let (_server, client) =
        answering(200, json!({"access_token": ACCESS, "token_type": "Bearer"})).await;

    let error = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("a grant without an expiry");

    assert!(
        matches!(error, LoginError::Malformed(UNREADABLE)),
        "{error:?}"
    );
    assert!(!format!("{error} {error:?}").contains(ACCESS), "{error}");
}

#[tokio::test]
async fn an_oversized_grant_is_refused_unread() {
    let (_server, client) = answering(
        200,
        json!({
            "access_token": ACCESS,
            "expires_in": GRANTED,
            "scope": "s".repeat(BODY_LIMIT),
        }),
    )
    .await;

    let error = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("past the limit");

    assert!(
        matches!(error, LoginError::Malformed(UNREADABLE)),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_transport_failure_that_names_its_cause() {
    let endpoints = Endpoints::default()
        .with_token("http://127.0.0.1:1/v1/oauth/token")
        .expect("endpoint");
    let client = Client::new(endpoints).expect("the client builds");

    let error = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("nothing listens on port 1");

    let LoginError::Transport(message) = &error else {
        panic!("expected a transport failure, got {error:?}");
    };
    assert!(message.contains("Connection refused"), "{message}");
    for sent in [CODE, VERIFIER, STATE, "127.0.0.1:1"] {
        assert!(!message.contains(sent), "{message}");
    }
}

#[test]
fn a_transport_failure_scrubs_whatever_the_request_carried() {
    let error = scrubbed(
        format!("error sending {CODE} with {VERIFIER} as sk-ant-oat01-fake0123456789abcdefghij"),
        &[CODE, VERIFIER, ""],
    );

    let LoginError::Transport(message) = &error else {
        panic!("expected a transport failure, got {error:?}");
    };
    assert_eq!(
        message,
        "error sending [REDACTED] with [REDACTED] as [REDACTED]"
    );
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/elsewhere", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/elsewhere"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": ACCESS, "expires_in": GRANTED})),
        )
        .expect(0)
        .mount(&server)
        .await;

    let error = client(&server)
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("a redirect is a rejection");

    assert!(
        matches!(
            error,
            LoginError::Rejected {
                status: 307,
                refusal: None,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn the_client_names_itself_as_this_crate() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .and(header(
            "user-agent",
            concat!("abnegate-agent-cli/", env!("CARGO_PKG_VERSION")),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": ACCESS, "expires_in": GRANTED})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tokens = client(&server)
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect("the user agent matched");

    assert_eq!(
        tokens.issued_at.map(|issued| tokens.expires_at - issued),
        Some(seconds(GRANTED))
    );
}

const UNRESOLVABLE: &str = "http://token.example.invalid/v1/oauth/token";

fn unresolvable() -> Endpoints {
    Endpoints::default()
        .with_token(UNRESOLVABLE)
        .expect("a token endpoint")
}

#[tokio::test]
async fn an_explicit_proxy_carries_every_request() {
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": ACCESS, "expires_in": GRANTED})),
        )
        .expect(1)
        .mount(&proxy)
        .await;
    let client = Client::new(unresolvable())
        .and_then(|client| client.with_proxy(&proxy.uri()))
        .expect("a loopback proxy");

    let tokens = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect("the proxy answered for the token endpoint");

    assert_eq!(tokens.access.expose(), ACCESS);
    assert_eq!(
        client.proxy().map(Url::as_str),
        Some(format!("{}/", proxy.uri()).as_str())
    );
    let requests = proxy
        .received_requests()
        .await
        .expect("the proxy records requests");
    assert_eq!(
        requests[0].url.as_str(),
        UNRESOLVABLE,
        "the proxy was not asked for the token endpoint"
    );
}

#[tokio::test]
async fn the_environments_proxy_is_never_consulted() {
    const NAME: &str = "login::claude::client::tests::the_environments_proxy_is_never_consulted";
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": ACCESS, "expires_in": GRANTED})),
        )
        .mount(&proxy)
        .await;
    let uri = proxy.uri();
    let variables = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .map(|variable| (variable, uri.as_str()));

    if delegated(NAME, &variables).await {
        assert_eq!(
            proxy
                .received_requests()
                .await
                .map(|requests| requests.len()),
            Some(0),
            "the environment's proxy was consulted"
        );
        return;
    }

    assert!(
        std::env::var_os("HTTP_PROXY").is_some(),
        "the child was given no proxy, so its run proves nothing"
    );
    let client = Client::new(unresolvable()).expect("the client builds");
    let error = client
        .exchange(&code(), &verifier(), Scope::Inference, Redirect::Paste)
        .await
        .expect_err("the token endpoint resolves nowhere");

    assert!(matches!(error, LoginError::Transport(_)), "{error:?}");
    assert!(client.proxy().is_none());
}

#[test]
fn a_proxy_is_checked_as_an_endpoint_is() {
    for proxy in [
        "http://169.254.169.254:3128",
        "http://user:secret@proxy.example:3128",
        "socks5://proxy.example:1080",
        "proxy.example:3128",
    ] {
        let refused = Client::new(Endpoints::default()).and_then(|client| client.with_proxy(proxy));

        match refused {
            Err(LoginError::Endpoint { name, reason }) => {
                assert_eq!(name, PROXY, "{proxy}");
                assert!(!reason.contains("secret"), "{proxy}: {reason}");
            }
            other => panic!("{proxy} must be refused, got {other:?}"),
        }
    }
    for proxy in [
        "http://127.0.0.1:3128",
        "http://10.0.0.5:3128",
        "https://proxy.example",
    ] {
        assert!(
            Client::new(Endpoints::default())
                .and_then(|client| client.with_proxy(proxy))
                .is_ok(),
            "{proxy}"
        );
    }
}
