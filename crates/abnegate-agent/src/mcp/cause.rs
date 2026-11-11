use std::error::Error;
use std::io;
use std::iter;

use abnegate_http::Error as Refusal;
use reqwest::StatusCode;
use rmcp::ServiceError;
use rmcp::service::ClientInitializeError;
use rmcp::transport::DynamicTransportError;
use rmcp::transport::streamable_http_client::StreamableHttpError;

use super::McpFailure;
use super::redirect::Redirect;

/// What failed a server's connection: its kind and, when an HTTP answer
/// failed it, that answer's status.
///
/// It is taken from an error's type and from what the error holds as data,
/// never from its text, which a server's reply and a bound secret both
/// shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cause {
    pub(super) failure: McpFailure,
    pub(super) status: Option<u16>,
}

impl Cause {
    /// `failure` with no HTTP answer behind it.
    pub(super) const fn of(failure: McpFailure) -> Self {
        Self {
            failure,
            status: None,
        }
    }

    /// `failure` from an error only an answer of `status` produces.
    const fn implied(failure: McpFailure, status: StatusCode) -> Self {
        Self {
            failure,
            status: Some(status.as_u16()),
        }
    }

    /// The cause behind a streamable HTTP request's `error`.
    pub(super) fn transport(error: &StreamableHttpError<reqwest::Error>) -> Self {
        match error {
            StreamableHttpError::Client(error) => Self::of(request(error)),
            StreamableHttpError::AuthRequired(_) => {
                Self::implied(McpFailure::Unauthorized, StatusCode::UNAUTHORIZED)
            }
            StreamableHttpError::InsufficientScope(_) => {
                Self::implied(McpFailure::Unauthorized, StatusCode::FORBIDDEN)
            }
            StreamableHttpError::HttpStatus(error) => Self {
                failure: McpFailure::Status,
                status: Some(error.status),
            },
            StreamableHttpError::SessionExpired => {
                Self::implied(McpFailure::Status, StatusCode::NOT_FOUND)
            }
            StreamableHttpError::UnexpectedServerResponse(_)
            | StreamableHttpError::UnexpectedContentType(_)
            | StreamableHttpError::Deserialize(_)
            | StreamableHttpError::Sse(_)
            | StreamableHttpError::MissingSessionIdInResponse => Self::of(McpFailure::Protocol),
            StreamableHttpError::ReinitializationFailed(_) => Self::of(McpFailure::Rejected),
            StreamableHttpError::ResponseBodyTooLarge { .. } => Self::of(McpFailure::Oversized),
            StreamableHttpError::TransportChannelClosed
            | StreamableHttpError::UnexpectedEndOfStream
            | StreamableHttpError::Io(_) => Self::of(McpFailure::Closed),
            StreamableHttpError::ReservedHeaderConflict(_) => Self::of(McpFailure::Header),
            StreamableHttpError::ControlRequestTimeout
            | StreamableHttpError::SessionRecoveryTimeout => Self::of(McpFailure::Timeout),
            _ => Self::of(McpFailure::Other),
        }
    }

    /// The cause behind a handshake's `error`.
    pub(super) fn initialize(error: &ClientInitializeError) -> Self {
        match error {
            ClientInitializeError::TransportError { error, .. } => Self::carried(error),
            ClientInitializeError::ConnectionClosed(_) => Self::of(McpFailure::Closed),
            ClientInitializeError::JsonRpcError(_) => Self::of(McpFailure::Rejected),
            ClientInitializeError::ExpectedInitResponse(_)
            | ClientInitializeError::ExpectedInitResult(_)
            | ClientInitializeError::ConflictInitResponseId(..)
            | ClientInitializeError::UncorrelatedErrorResponse { .. }
            | ClientInitializeError::NoCompatibleProtocolVersion { .. }
            | ClientInitializeError::NoPreferredProtocolVersion => Self::of(McpFailure::Protocol),
            _ => Self::of(McpFailure::Other),
        }
    }

    /// The cause behind the `error` of a request made once the handshake
    /// was done, such as the tool listing.
    pub(super) fn service(error: &ServiceError) -> Self {
        match error {
            ServiceError::TransportSend(error) => Self::carried(error),
            ServiceError::TransportClosed => Self::of(McpFailure::Closed),
            ServiceError::McpError(_) => Self::of(McpFailure::Rejected),
            ServiceError::UnexpectedResponse => Self::of(McpFailure::Protocol),
            ServiceError::Timeout { .. } => Self::of(McpFailure::Timeout),
            _ => Self::of(McpFailure::Other),
        }
    }

    /// The cause behind the error a transport of either kind gave: a
    /// streamable HTTP request's own, or the `io::Error` of a stdio pipe.
    fn carried(error: &DynamicTransportError) -> Self {
        let error = error.error.as_ref();
        if let Some(error) = error.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
            return Self::transport(error);
        }
        if error.is::<io::Error>() {
            return Self::of(McpFailure::Closed);
        }
        Self::of(McpFailure::Other)
    }
}

/// The kind of a request the HTTP client itself failed. A refused redirect
/// is the request's direct source; a name that resolved only to link-local
/// or metadata addresses lies deeper, under the connection it stopped.
fn request(error: &reqwest::Error) -> McpFailure {
    if error.source().is_some_and(<dyn Error>::is::<Redirect>) {
        return McpFailure::Redirect;
    }
    let refused = iter::successors(error.source(), |source| (*source).source()).any(|source| {
        matches!(
            source.downcast_ref::<Refusal>(),
            Some(Refusal::MetadataResolution { .. })
        )
    });
    if refused {
        return McpFailure::Metadata;
    }
    if error.is_timeout() {
        return McpFailure::Timeout;
    }
    if error.is_body() || error.is_decode() {
        return McpFailure::Closed;
    }
    McpFailure::Unreachable
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::fmt;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use abnegate_http::metadata::MetadataResolver;
    use reqwest::dns::Name;
    use reqwest::dns::Resolve;
    use reqwest::dns::Resolving;
    use reqwest::redirect::Policy;
    use rmcp::ErrorData;
    use rmcp::model::RequestId;
    use rmcp::transport::streamable_http_client::AuthRequiredError;
    use rmcp::transport::streamable_http_client::HttpStatusError;
    use rmcp::transport::streamable_http_client::InsufficientScopeError;
    use rmcp::transport::streamable_http_client::SseError;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;

    use super::*;

    type Failure = StreamableHttpError<reqwest::Error>;

    const CHALLENGE: &str = "Bearer realm=\"mcp\"";

    /// A name no resolver is asked for but the one a test installs.
    const UNRESOLVABLE: &str = "http://mcp.invalid/mcp";

    fn answered(failure: McpFailure, status: u16) -> Cause {
        Cause {
            failure,
            status: Some(status),
        }
    }

    fn rejection() -> ErrorData {
        ErrorData::internal_error("no", None)
    }

    fn broken() -> io::Error {
        io::Error::from(io::ErrorKind::BrokenPipe)
    }

    fn undecodable() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{").expect_err("not JSON")
    }

    /// `error` as a transport of any kind hands it to a session.
    fn carried(error: impl Error + Send + Sync + 'static) -> DynamicTransportError {
        DynamicTransportError::from_parts("test", TypeId::of::<()>(), Box::new(error))
    }

    fn client() -> reqwest::ClientBuilder {
        reqwest::Client::builder().no_proxy()
    }

    /// Answers every name with what its function resolves.
    struct Scripted(fn() -> Resolving);

    impl Resolve for Scripted {
        fn resolve(&self, _: Name) -> Resolving {
            (self.0)()
        }
    }

    /// What [`MetadataResolver`] answers for a metadata address, which it
    /// refuses without asking the network.
    fn metadata() -> Resolving {
        MetadataResolver::new().resolve("169.254.169.254".parse().expect("a name"))
    }

    fn nameless() -> Resolving {
        Box::pin(async { Err(io::Error::from(io::ErrorKind::NotFound).into()) })
    }

    /// The error of a request whose name `resolver` answers.
    async fn unresolved(resolver: Scripted) -> Failure {
        let error = client()
            .dns_resolver(resolver)
            .build()
            .expect("a client")
            .get(UNRESOLVABLE)
            .send()
            .await
            .expect_err("the name resolves to nothing reachable");
        StreamableHttpError::Client(error)
    }

    /// A local listener and the URL that reaches it.
    async fn listening() -> (TcpListener, String) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("a listener");
        let url = format!("http://{}/mcp", listener.local_addr().expect("an address"));
        (listener, url)
    }

    #[test]
    fn a_cause_of_a_kind_alone_has_no_status() {
        assert_eq!(
            Cause::of(McpFailure::Spawn),
            Cause {
                failure: McpFailure::Spawn,
                status: None
            }
        );
    }

    #[test]
    fn a_challenge_is_unauthorized_with_the_status_that_carries_it() {
        let required = Failure::AuthRequired(AuthRequiredError::new(CHALLENGE.to_string()));
        let insufficient =
            Failure::InsufficientScope(InsufficientScopeError::new(CHALLENGE.to_string(), None));

        assert_eq!(
            Cause::transport(&required),
            answered(McpFailure::Unauthorized, 401)
        );
        assert_eq!(
            Cause::transport(&insufficient),
            answered(McpFailure::Unauthorized, 403)
        );
    }

    #[test]
    fn a_failing_status_is_carried_as_data() {
        for status in [429, 500, 503] {
            let error = Failure::HttpStatus(HttpStatusError::new(status, None));

            assert_eq!(
                Cause::transport(&error),
                answered(McpFailure::Status, status)
            );
        }
        assert_eq!(
            Cause::transport(&Failure::SessionExpired),
            answered(McpFailure::Status, 404)
        );
    }

    #[test]
    fn an_answer_that_is_not_the_protocol_is_a_protocol_failure() {
        let errors = [
            Failure::UnexpectedServerResponse("HTTP 503: Auth required".into()),
            Failure::UnexpectedContentType(Some("text/html".to_string())),
            Failure::UnexpectedContentType(None),
            Failure::Deserialize(undecodable()),
            Failure::Sse(SseError::InvalidLine),
            Failure::MissingSessionIdInResponse,
        ];

        for error in errors {
            assert_eq!(
                Cause::transport(&error),
                Cause::of(McpFailure::Protocol),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_refused_reinitialization_is_rejected() {
        assert_eq!(
            Cause::transport(&Failure::ReinitializationFailed(rejection())),
            Cause::of(McpFailure::Rejected)
        );
    }

    #[test]
    fn a_body_past_the_limit_is_oversized() {
        assert_eq!(
            Cause::transport(&Failure::ResponseBodyTooLarge { max_size: 1 }),
            Cause::of(McpFailure::Oversized)
        );
    }

    #[test]
    fn a_stream_that_ends_is_closed() {
        let errors = [
            Failure::TransportChannelClosed,
            Failure::UnexpectedEndOfStream,
            Failure::Io(broken()),
        ];

        for error in errors {
            assert_eq!(
                Cause::transport(&error),
                Cause::of(McpFailure::Closed),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_reserved_header_is_a_header_failure() {
        assert_eq!(
            Cause::transport(&Failure::ReservedHeaderConflict("accept".to_string())),
            Cause::of(McpFailure::Header)
        );
    }

    #[test]
    fn a_transport_that_gives_up_waiting_timed_out() {
        for error in [
            Failure::ControlRequestTimeout,
            Failure::SessionRecoveryTimeout,
        ] {
            assert_eq!(
                Cause::transport(&error),
                Cause::of(McpFailure::Timeout),
                "{error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_transport_error_with_no_kind_of_its_own_is_other() {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let aborted = task.await.expect_err("the task was aborted");
        let errors = [
            Failure::TokioJoinError(aborted),
            Failure::ServerDoesNotSupportSse,
            Failure::ServerDoesNotSupportDeleteSession,
        ];

        for error in errors {
            assert_eq!(
                Cause::transport(&error),
                Cause::of(McpFailure::Other),
                "{error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_redirect_is_a_redirect() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
            .mount(&server)
            .await;
        let error = client()
            .redirect(Policy::custom(|attempt| attempt.error(Redirect::Exceeded)))
            .build()
            .expect("a client")
            .get(server.uri())
            .send()
            .await
            .expect_err("the redirect is refused");

        assert_eq!(
            Cause::transport(&Failure::Client(error)),
            Cause::of(McpFailure::Redirect)
        );
    }

    /// The resolver's refusal lies under the connection error it caused by
    /// the time a request fails with it, and is still there to find by type.
    #[tokio::test]
    async fn a_name_that_resolves_only_to_metadata_addresses_is_metadata() {
        let error = unresolved(Scripted(metadata)).await;

        assert_eq!(Cause::transport(&error), Cause::of(McpFailure::Metadata));
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_unreachable() {
        let error = unresolved(Scripted(nameless)).await;

        assert_eq!(Cause::transport(&error), Cause::of(McpFailure::Unreachable));
    }

    #[tokio::test]
    async fn a_refused_connection_is_unreachable() {
        let error = client()
            .build()
            .expect("a client")
            .get("http://127.0.0.1:1/mcp")
            .send()
            .await
            .expect_err("nothing listens on port 1");

        assert_eq!(
            Cause::transport(&Failure::Client(error)),
            Cause::of(McpFailure::Unreachable)
        );
    }

    #[tokio::test]
    async fn a_request_that_outlasts_its_client_timed_out() {
        let (_listener, url) = listening().await;
        let error = client()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("a client")
            .get(url)
            .send()
            .await
            .expect_err("no answer comes");

        assert_eq!(
            Cause::transport(&Failure::Client(error)),
            Cause::of(McpFailure::Timeout)
        );
    }

    #[tokio::test]
    async fn a_body_cut_short_is_closed() {
        const PARTIAL: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 64\r\n\r\npartial";
        let (listener, url) = listening().await;
        let served = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("a connection");
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.expect("a request");
            stream.write_all(PARTIAL).await.expect("a partial answer");
        });
        let response = client()
            .build()
            .expect("a client")
            .get(url)
            .send()
            .await
            .expect("the head arrives");
        let error = response
            .bytes()
            .await
            .expect_err("the body ends before its length");
        served.await.expect("the server ran");

        assert_eq!(
            Cause::transport(&Failure::Client(error)),
            Cause::of(McpFailure::Closed)
        );
    }

    #[tokio::test]
    async fn a_body_that_does_not_decode_is_closed() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("{"))
            .mount(&server)
            .await;
        let error = client()
            .build()
            .expect("a client")
            .get(server.uri())
            .send()
            .await
            .expect("an answer")
            .json::<serde_json::Value>()
            .await
            .expect_err("not JSON");

        assert_eq!(
            Cause::transport(&Failure::Client(error)),
            Cause::of(McpFailure::Closed)
        );
    }

    #[test]
    fn a_handshake_answered_out_of_turn_is_a_protocol_failure() {
        let errors = [
            ClientInitializeError::ExpectedInitResponse(None),
            ClientInitializeError::ExpectedInitResult(None),
            ClientInitializeError::ConflictInitResponseId(
                RequestId::Number(1),
                RequestId::Number(2),
            ),
            ClientInitializeError::UncorrelatedErrorResponse {
                expected: RequestId::Number(1),
                received: RequestId::Number(2),
            },
            ClientInitializeError::NoCompatibleProtocolVersion {
                client_supported: Vec::new(),
                server_supported: Vec::new(),
            },
            ClientInitializeError::NoPreferredProtocolVersion,
        ];

        for error in errors {
            assert_eq!(
                Cause::initialize(&error),
                Cause::of(McpFailure::Protocol),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_handshake_whose_connection_went_is_closed() {
        let error = ClientInitializeError::ConnectionClosed("initialize response".to_string());

        assert_eq!(Cause::initialize(&error), Cause::of(McpFailure::Closed));
    }

    #[test]
    fn a_handshake_answered_with_an_error_is_rejected() {
        let error = ClientInitializeError::JsonRpcError(rejection());

        assert_eq!(Cause::initialize(&error), Cause::of(McpFailure::Rejected));
    }

    #[test]
    fn a_cancelled_handshake_is_other() {
        assert_eq!(
            Cause::initialize(&ClientInitializeError::Cancelled),
            Cause::of(McpFailure::Other)
        );
    }

    #[test]
    fn a_handshake_takes_the_cause_its_transport_carried() {
        let cases = [
            (
                carried(Failure::HttpStatus(HttpStatusError::new(503, None))),
                answered(McpFailure::Status, 503),
            ),
            (
                carried(Failure::AuthRequired(AuthRequiredError::new(
                    CHALLENGE.to_string(),
                ))),
                answered(McpFailure::Unauthorized, 401),
            ),
            (carried(broken()), Cause::of(McpFailure::Closed)),
            (carried(fmt::Error), Cause::of(McpFailure::Other)),
        ];

        for (error, cause) in cases {
            let error = ClientInitializeError::TransportError {
                error,
                context: "send initialize request".into(),
            };

            assert_eq!(Cause::initialize(&error), cause, "{error:?}");
        }
    }

    #[test]
    fn a_request_takes_the_cause_its_transport_carried() {
        let cases = [
            (
                carried(Failure::InsufficientScope(InsufficientScopeError::new(
                    CHALLENGE.to_string(),
                    None,
                ))),
                answered(McpFailure::Unauthorized, 403),
            ),
            (
                carried(Failure::SessionExpired),
                answered(McpFailure::Status, 404),
            ),
            (carried(broken()), Cause::of(McpFailure::Closed)),
            (carried(fmt::Error), Cause::of(McpFailure::Other)),
        ];

        for (error, cause) in cases {
            let error = ServiceError::TransportSend(error);

            assert_eq!(Cause::service(&error), cause, "{error:?}");
        }
    }

    #[test]
    fn a_request_names_what_became_of_it() {
        let cases = [
            (ServiceError::McpError(rejection()), McpFailure::Rejected),
            (ServiceError::TransportClosed, McpFailure::Closed),
            (ServiceError::UnexpectedResponse, McpFailure::Protocol),
            (
                ServiceError::Timeout {
                    timeout: Duration::from_secs(1),
                },
                McpFailure::Timeout,
            ),
            (ServiceError::Cancelled { reason: None }, McpFailure::Other),
            (
                ServiceError::SubscriptionLagged { capacity: 1 },
                McpFailure::Other,
            ),
            (
                ServiceError::InputRequiredRoundsExceeded { max_rounds: 1 },
                McpFailure::Other,
            ),
        ];

        for (error, failure) in cases {
            assert_eq!(Cause::service(&error), Cause::of(failure), "{error:?}");
        }
    }
}
