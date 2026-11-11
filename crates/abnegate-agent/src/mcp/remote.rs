use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use abnegate_exec::Proxy;
use abnegate_http::metadata::MetadataResolver;
use reqwest::NoProxy;
use reqwest::StatusCode;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::model::ClientNotification;
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;

use super::cause::Cause;
use super::cause::failing;
use super::posted::Posted;
use super::posted::Slot;
use super::redirect::MAXIMUM_REDIRECTS;
use super::redirect::Redirect;
use super::retry::answered;
use super::retry::retries;
use super::scrubber::Scrubber;

const INVALID_PROXY: &str = "invalid proxy URL";

const CLIENT_FAILED: &str = "could not build an HTTP client";

type Failure = StreamableHttpError<reqwest::Error>;

/// The HTTP client one remote server is reached with: it resolves names past
/// link-local and metadata addresses, follows only the redirects
/// [`redirects`] allows, retries only as [`retries`] says, which is as a
/// client told nothing does, and routes through the hub's proxy alone,
/// never a proxy the environment or the system names.
///
/// Every error it gives the transport is stripped of the URL it was sent
/// to, which may hold a resolved secret in its path or query, and has its
/// text scrubbed of the server's secrets, a redirect's location and a
/// server's own words included, since the transport logs some of them
/// itself before any caller sees them.
///
/// That rewriting can cost an error its kind, and the transport keeps an
/// answer's status only in an error's text, so what became of the last
/// [recorded](recorded) request is kept first, with the status the client
/// saw it [`answered`] with, as a [`Posted`] every clone shares with
/// whoever asked for [`posted`](Self::posted).
#[derive(Clone)]
pub(super) struct Remote {
    client: reqwest::Client,
    scrubber: Scrubber,
    posted: Arc<Slot>,
}

impl Remote {
    /// A client routed through `proxy` whose errors `scrubber` scrubs, or
    /// why none could be built.
    pub(super) fn new(proxy: &Proxy, scrubber: Scrubber) -> Result<Self, String> {
        let builder = reqwest::Client::builder()
            .dns_resolver(MetadataResolver::new())
            .redirect(redirects())
            .retry(retries());
        let builder = match proxy.url() {
            Some(url) => {
                let url = url.to_str().ok_or(INVALID_PROXY)?;
                let route = reqwest::Proxy::all(url)
                    .map_err(|_| INVALID_PROXY)?
                    .no_proxy(NoProxy::from_string(&proxy.bypass().join(",")));
                builder.proxy(route)
            }
            None => builder.no_proxy(),
        };
        builder
            .build()
            .map(|client| Self {
                client,
                scrubber,
                posted: Arc::default(),
            })
            .map_err(|error| format!("{CLIENT_FAILED}: {}", error.without_url()))
    }

    /// Where this client keeps what became of the last [recorded](recorded)
    /// request it sent, to be read once a request the session waited on has
    /// failed, never before. What it writes there once the session has
    /// connected is never read.
    pub(super) fn posted(&self) -> Arc<Slot> {
        self.posted.clone()
    }
}

impl Remote {
    /// The answer `post` gets, its error [unlocated](Self::unlocated).
    ///
    /// When `recorded`, what became of the request before it is forgotten
    /// as it is sent, and what became of this one is kept as it finishes,
    /// taken from the error before that is rewritten and from the status
    /// the client saw this request, and no other, answered with. Any other
    /// message leaves what is kept alone, so one sent beside the request a
    /// session waits on never stands in for what became of that request.
    async fn sent(
        &self,
        recorded: bool,
        post: impl Future<Output = Result<StreamableHttpPostResponse, Failure>> + Send,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        if recorded {
            Posted::default().write(&self.posted);
        }
        let (result, answer) = answered(post).await;
        if recorded {
            kept(&result, answer).write(&self.posted);
        }
        result.map_err(|error| self.unlocated(error))
    }

    /// `error` with no URL or secret in it: a refused redirect becomes the
    /// [`Redirect`] that refused it, any other client error drops the URL it
    /// names, and an error whose text holds a secret becomes that text
    /// scrubbed. One that holds none is passed on as it is, so the transport
    /// still tells its kinds apart.
    fn unlocated(&self, error: Failure) -> Failure {
        let error = match error {
            StreamableHttpError::Client(error) => {
                match Error::source(&error).and_then(|source| source.downcast_ref::<Redirect>()) {
                    Some(redirect) => {
                        StreamableHttpError::UnexpectedServerResponse(redirect.to_string().into())
                    }
                    None => StreamableHttpError::Client(error.without_url()),
                }
            }
            error => error,
        };
        if let StreamableHttpError::UnexpectedServerResponse(text) = &error {
            return StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                self.scrubber.scrubbed(text),
            ));
        }
        let text = error.to_string();
        let scrubbed = self.scrubber.scrubbed(&text);
        if scrubbed == text {
            error
        } else {
            StreamableHttpError::UnexpectedServerResponse(Cow::Owned(scrubbed))
        }
    }
}

impl StreamableHttpClient for Remote {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        let recorded = recorded(&message);
        let post = self
            .client
            .post_message(uri, message, session_id, auth_header, custom_headers);
        self.sent(recorded, post).await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        let recorded = recorded(&message);
        let post = self.client.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            max_sse_event_size,
        );
        self.sent(recorded, post).await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Failure> {
        self.client
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
            .map_err(|error| self.unlocated(error))
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, Failure> {
        self.client
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
            .map_err(|error| self.unlocated(error))
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxedSseResponse, Failure> {
        self.client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
            .map_err(|error| self.unlocated(error))
    }
}

/// Whether `message` is one a connecting session waits on, whose fate is
/// therefore kept: a request, or the notification that ends the handshake.
/// Those are sent one at a time while a session connects. A response, an
/// error, a cancellation or any other notification goes out beside them.
fn recorded(message: &ClientJsonRpcMessage) -> bool {
    match message {
        ClientJsonRpcMessage::Request(_) => true,
        ClientJsonRpcMessage::Notification(notification) => matches!(
            notification.notification,
            ClientNotification::InitializedNotification(_)
        ),
        ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_) => false,
    }
}

/// What became of the request `result` answers, which the client saw
/// answered with `answer`, if at all. An answer that was not a success
/// keeps its status though the transport made something of it, as it does
/// of one carrying a JSON-RPC error.
fn kept(
    result: &Result<StreamableHttpPostResponse, Failure>,
    answer: Option<StatusCode>,
) -> Posted {
    match result {
        Ok(_) => Posted {
            failure: None,
            status: failing(answer),
        },
        Err(error) => {
            let Cause { failure, status } = Cause::transport(error, answer);
            Posted {
                failure: Some(failure),
                status,
            }
        }
    }
}

/// Follow a 307 or 308, which keep the method and body, while it stays on
/// the origin first requested, for at most [`MAXIMUM_REDIRECTS`] hops, and
/// refuse any other with the [`Redirect`] that says why: where it pointed,
/// as its origin and path, or that there were too many.
fn redirects() -> Policy {
    Policy::custom(|attempt| {
        let hops = attempt.previous().len();
        let preserving = matches!(
            attempt.status(),
            StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
        );
        let within = attempt
            .previous()
            .first()
            .is_some_and(|sent| sent.origin() == attempt.url().origin());
        if !preserving || !within {
            let location = format!(
                "{}{}",
                attempt.url().origin().ascii_serialization(),
                attempt.url().path()
            );
            return attempt.error(Redirect::Refused(location));
        }
        if hops > MAXIMUM_REDIRECTS {
            return attempt.error(Redirect::Exceeded);
        }
        tracing::debug!(
            status = %attempt.status(),
            hop = hops,
            "following a same-origin MCP redirect"
        );
        attempt.follow()
    })
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use serde_json::json;
    use tokio::net::TcpListener;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;

    use super::*;
    use crate::mcp::McpFailure;
    use crate::mcp::McpServer;
    use crate::mcp::fixture::MCP_PATH;

    const SESSION: &str = "session";

    /// What a refused request answered 503 leaves behind.
    const UNAVAILABLE: Posted = Posted {
        failure: Some(McpFailure::Status),
        status: Some(503),
    };

    fn message(json: serde_json::Value) -> ClientJsonRpcMessage {
        serde_json::from_value(json).expect("a client message")
    }

    fn request() -> ClientJsonRpcMessage {
        message(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
    }

    fn initialized() -> ClientJsonRpcMessage {
        message(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
    }

    fn response() -> ClientJsonRpcMessage {
        message(json!({"jsonrpc": "2.0", "id": 1, "result": {}}))
    }

    fn rejection() -> ClientJsonRpcMessage {
        message(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32603, "message": "no"}
        }))
    }

    fn cancellation() -> ClientJsonRpcMessage {
        message(json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": 1}
        }))
    }

    fn changed() -> ClientJsonRpcMessage {
        message(json!({"jsonrpc": "2.0", "method": "notifications/roots/list_changed"}))
    }

    /// Every message a session sends beside the request it waits on.
    fn beside() -> [ClientJsonRpcMessage; 4] {
        [response(), rejection(), cancellation(), changed()]
    }

    fn remote() -> Remote {
        Remote::new(&Proxy::default(), Scrubber::default()).expect("a client")
    }

    fn address(server: &MockServer) -> Arc<str> {
        format!("{}{MCP_PATH}", server.uri()).into()
    }

    /// Have `server` answer everything with `template` from now on.
    async fn answering(server: &MockServer, template: ResponseTemplate) {
        server.reset().await;
        Mock::given(any())
            .respond_with(template)
            .mount(server)
            .await;
    }

    async fn post(
        remote: &Remote,
        server: &MockServer,
        message: ClientJsonRpcMessage,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        remote
            .post_message(address(server), message, None, None, HashMap::new())
            .await
    }

    /// Leave [`UNAVAILABLE`] behind in what `remote` keeps.
    async fn refuse(remote: &Remote, server: &MockServer) {
        answering(server, ResponseTemplate::new(503)).await;
        post(remote, server, request())
            .await
            .expect_err("a 503 fails the request");
        assert_eq!(Posted::read(&remote.posted()), UNAVAILABLE);
    }

    fn challenged() -> ResponseTemplate {
        ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Bearer realm=\"mcp\"")
    }

    fn redirecting() -> ResponseTemplate {
        ResponseTemplate::new(302).insert_header("Location", "/elsewhere")
    }

    #[test]
    fn only_a_request_and_the_end_of_the_handshake_are_recorded() {
        assert!(matches!(request(), ClientJsonRpcMessage::Request(_)));
        assert!(matches!(response(), ClientJsonRpcMessage::Response(_)));
        assert!(matches!(rejection(), ClientJsonRpcMessage::Error(_)));
        for notification in [initialized(), cancellation(), changed()] {
            assert!(
                matches!(notification, ClientJsonRpcMessage::Notification(_)),
                "{notification:?}"
            );
        }

        assert!(recorded(&request()));
        assert!(recorded(&initialized()));
        for message in beside() {
            assert!(!recorded(&message), "{message:?}");
        }
    }

    #[tokio::test]
    async fn a_request_that_fails_leaves_why_and_one_answered_forgets_it() {
        let server = MockServer::start().await;
        let remote = remote();
        refuse(&remote, &server).await;

        answering(&server, ResponseTemplate::new(202)).await;
        post(&remote, &server, request())
            .await
            .expect("a 202 is an answer");

        assert_eq!(Posted::read(&remote.posted()), Posted::default());
    }

    #[tokio::test]
    async fn the_end_of_the_handshake_is_recorded_as_a_request_is() {
        let server = MockServer::start().await;
        let remote = remote();

        answering(&server, challenged()).await;
        post(&remote, &server, initialized())
            .await
            .expect_err("a challenge fails the notification");
        assert_eq!(
            Posted::read(&remote.posted()),
            Posted {
                failure: Some(McpFailure::Unauthorized),
                status: Some(401)
            }
        );

        answering(&server, ResponseTemplate::new(202)).await;
        post(&remote, &server, initialized())
            .await
            .expect("a 202 is an answer");
        assert_eq!(Posted::read(&remote.posted()), Posted::default());
    }

    /// A response, an error, a cancellation or another notification goes out
    /// beside the request a session waits on, so neither sending one nor its
    /// answer may stand in for what became of that request.
    #[tokio::test]
    async fn a_message_sent_beside_a_request_leaves_what_was_posted_alone() {
        let server = MockServer::start().await;
        let remote = remote();
        refuse(&remote, &server).await;

        for status in [202, 500] {
            answering(&server, ResponseTemplate::new(status)).await;
            for message in beside() {
                let sent = format!("{message:?}");
                let result = post(&remote, &server, message).await;

                assert_eq!(result.is_ok(), status == 202, "{sent} answered {status}");
                assert_eq!(
                    Posted::read(&remote.posted()),
                    UNAVAILABLE,
                    "{sent} answered {status}"
                );
            }
        }
    }

    #[tokio::test]
    async fn the_size_limited_post_records_as_the_plain_one_does() {
        let server = MockServer::start().await;
        let remote = remote();
        let post = |message: ClientJsonRpcMessage| {
            remote.post_message_with_max_sse_event_size(
                address(&server),
                message,
                None,
                None,
                HashMap::new(),
                usize::MAX,
            )
        };

        answering(&server, ResponseTemplate::new(503)).await;
        post(request()).await.expect_err("a 503 fails the request");
        assert_eq!(Posted::read(&remote.posted()), UNAVAILABLE);

        answering(&server, ResponseTemplate::new(500)).await;
        for message in beside() {
            post(message).await.expect_err("a 500 fails the message");
            assert_eq!(Posted::read(&remote.posted()), UNAVAILABLE);
        }

        answering(&server, ResponseTemplate::new(202)).await;
        post(request()).await.expect("a 202 is an answer");
        assert_eq!(Posted::read(&remote.posted()), Posted::default());
    }

    /// The background stream and the session's deletion are no request a
    /// session waits on, and a refusal either meets must never be read as
    /// why that request failed.
    #[tokio::test]
    async fn a_stream_or_a_deletion_never_touches_what_was_posted() {
        let server = MockServer::start().await;
        let remote = remote();

        for seeded in [false, true] {
            let kept = if seeded {
                refuse(&remote, &server).await;
                UNAVAILABLE
            } else {
                Posted::default()
            };
            for template in [ResponseTemplate::new(500), redirecting(), challenged()] {
                answering(&server, template).await;
                let stream = remote
                    .get_stream(
                        address(&server),
                        Some(SESSION.into()),
                        None,
                        None,
                        HashMap::new(),
                    )
                    .await;
                let limited = remote
                    .get_stream_with_max_sse_event_size(
                        address(&server),
                        Some(SESSION.into()),
                        None,
                        None,
                        HashMap::new(),
                        usize::MAX,
                    )
                    .await;
                let deleted = remote
                    .delete_session(address(&server), SESSION.into(), None, HashMap::new())
                    .await;

                assert!(stream.is_err() && limited.is_err() && deleted.is_err());
                assert_eq!(Posted::read(&remote.posted()), kept);
            }
        }
    }

    /// A request still in flight has no fate yet, and the one before it is
    /// no longer what the session waits on.
    #[tokio::test]
    async fn a_request_forgets_the_one_before_it_as_it_is_sent() {
        let server = MockServer::start().await;
        let remote = remote();
        refuse(&remote, &server).await;
        let silent = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("a listener");
        let unanswered: Arc<str> = format!(
            "http://{}{MCP_PATH}",
            silent.local_addr().expect("an address")
        )
        .into();
        let sender = remote.clone();
        let sending = tokio::spawn(async move {
            sender
                .post_message(unanswered, request(), None, None, HashMap::new())
                .await
        });

        let _held = silent.accept().await.expect("the request's connection");

        assert_eq!(Posted::read(&remote.posted()), Posted::default());
        sending.abort();
    }

    /// An error whose text holds a bound secret, and a refused redirect, are
    /// both handed on as an unexpected response, with nothing left of what
    /// they were but what was kept before that.
    #[tokio::test]
    async fn what_was_posted_is_taken_before_the_error_is_rewritten() {
        let server = MockServer::start().await;
        let bound =
            McpServer::remote(address(&server).to_string()).with_secret("TOKEN", "HTTP 503");
        let endpoint = bound.endpoint().expect("an endpoint");
        let remote =
            Remote::new(&Proxy::default(), Scrubber::new(&bound, &endpoint)).expect("a client");
        let cases = [
            (ResponseTemplate::new(503), UNAVAILABLE),
            (
                redirecting(),
                Posted {
                    failure: Some(McpFailure::Redirect),
                    status: Some(302),
                },
            ),
        ];

        for (template, kept) in cases {
            answering(&server, template).await;
            let error = post(&remote, &server, request())
                .await
                .expect_err("the request fails");

            assert!(
                matches!(error, StreamableHttpError::UnexpectedServerResponse(_)),
                "{error:?}"
            );
            assert_eq!(Posted::read(&remote.posted()), kept);
        }
    }

    /// The transport has no kind for most answers that are not a success,
    /// a 401 or a 403 that came with no challenge included, and keeps their
    /// status only in the text of its error.
    #[tokio::test]
    async fn an_answer_the_transport_has_no_kind_for_leaves_its_status() {
        let server = MockServer::start().await;
        let remote = remote();

        for status in [400, 401, 403, 404, 405] {
            answering(&server, ResponseTemplate::new(status)).await;
            post(&remote, &server, request())
                .await
                .expect_err("the request fails");

            assert_eq!(
                Posted::read(&remote.posted()),
                Posted {
                    failure: Some(McpFailure::Status),
                    status: Some(status)
                }
            );
        }
    }

    /// A JSON-RPC error is an answer to the transport whatever status
    /// carried it, so the request does not fail, and the session that
    /// fails over it has only this to tell the status by.
    #[tokio::test]
    async fn an_answer_the_transport_took_leaves_its_status_unless_it_was_a_success() {
        let server = MockServer::start().await;
        let remote = remote();
        let rejected = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32603, "message": "no"}
        });

        for (status, kept) in [(400, Some(400)), (500, Some(500)), (200, None)] {
            answering(
                &server,
                ResponseTemplate::new(status).set_body_json(&rejected),
            )
            .await;
            let answer = post(&remote, &server, request())
                .await
                .expect("a JSON-RPC error is an answer");

            assert!(
                matches!(answer, StreamableHttpPostResponse::Json(..)),
                "{answer:?}"
            );
            assert_eq!(
                Posted::read(&remote.posted()),
                Posted {
                    failure: None,
                    status: kept
                }
            );
        }
    }

    #[tokio::test]
    async fn a_success_the_client_could_not_use_leaves_no_status() {
        let server = MockServer::start().await;
        let remote = remote();
        answering(
            &server,
            ResponseTemplate::new(200).set_body_raw("<html></html>", "text/html"),
        )
        .await;

        post(&remote, &server, request())
            .await
            .expect_err("a page is no answer");

        assert_eq!(
            Posted::read(&remote.posted()),
            Posted {
                failure: Some(McpFailure::Protocol),
                status: None
            }
        );
    }

    /// The transport sends a session's other messages and opens its stream
    /// from the task that sends its requests, so the status a request
    /// leaves must be its own answer's and none of theirs.
    #[tokio::test]
    async fn a_request_sent_beside_others_in_one_task_leaves_only_its_own_answer() {
        let server = MockServer::start().await;
        let other = MockServer::start().await;
        let remote = remote();
        answering(&server, ResponseTemplate::new(404)).await;
        answering(&other, ResponseTemplate::new(500)).await;

        let (requested, beside, stream) = tokio::join!(
            post(&remote, &server, request()),
            post(&remote, &other, response()),
            remote.get_stream(
                address(&other),
                Some(SESSION.into()),
                None,
                None,
                HashMap::new()
            ),
        );

        assert!(requested.is_err() && beside.is_err() && stream.is_err());
        assert_eq!(
            Posted::read(&remote.posted()),
            Posted {
                failure: Some(McpFailure::Status),
                status: Some(404)
            }
        );
    }
}
