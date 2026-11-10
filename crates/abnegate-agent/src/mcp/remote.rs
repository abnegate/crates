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
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;

use super::redirect::MAXIMUM_REDIRECTS;
use super::redirect::Redirect;

const INVALID_PROXY: &str = "invalid proxy URL";

const CLIENT_FAILED: &str = "could not build an HTTP client";

type Failure = StreamableHttpError<reqwest::Error>;

/// The HTTP client one remote server is reached with: it resolves names past
/// link-local and metadata addresses, follows only the redirects
/// [`redirects`] allows, and routes through the hub's proxy alone, never a
/// proxy the environment or the system names.
///
/// Every error it gives the transport is stripped of the URL it was sent
/// to, which may hold a resolved secret in its path or query, percent-encoded
/// or not, since the transport logs some of them itself.
#[derive(Clone)]
pub(super) struct Remote(reqwest::Client);

impl Remote {
    /// A client routed through `proxy`, or why none could be built.
    pub(super) fn new(proxy: &Proxy) -> Result<Self, String> {
        let builder = reqwest::Client::builder()
            .dns_resolver(MetadataResolver::new())
            .redirect(redirects());
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
            .map(Self)
            .map_err(|error| format!("{CLIENT_FAILED}: {}", error.without_url()))
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
        self.0
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await
            .map_err(unlocated)
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
        self.0
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
            .map_err(unlocated)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Failure> {
        self.0
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
            .map_err(unlocated)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, Failure> {
        self.0
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
            .map_err(unlocated)
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
        self.0
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
            .map_err(unlocated)
    }
}

/// `error` with no URL in it: a refused redirect becomes the [`Redirect`]
/// that refused it, and any other client error drops the URL it names.
fn unlocated(error: Failure) -> Failure {
    let StreamableHttpError::Client(error) = error else {
        return error;
    };
    match Error::source(&error).and_then(|source| source.downcast_ref::<Redirect>()) {
        Some(redirect) => {
            StreamableHttpError::UnexpectedServerResponse(Cow::Owned(redirect.to_string()))
        }
        None => StreamableHttpError::Client(error.without_url()),
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
