/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong issuing or validating an HTTP request.
///
/// A variant may gain a field in a minor release, so an
/// [`HttpClient`](crate::HttpClient) implemented outside this crate, a test
/// double included, fails through a constructor rather than a literal:
/// [`Error::unfetchable_resolution`] or [`Error::oversized_body`]. A pattern
/// outside this crate ends in `..`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The underlying transport refused or failed the request.
    ///
    /// Converting a [`reqwest::Error`] strips the URL it carries, whose query
    /// string often holds a key or a token.
    #[error(transparent)]
    Request(reqwest::Error),

    /// A response body did not deserialise into the requested type.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    /// The client implementation does not offer this method.
    #[error("{0} is not supported by this HTTP client")]
    Unsupported(reqwest::Method),

    /// The URL did not parse.
    #[error("Invalid URL.")]
    InvalidUrl,

    /// A header name did not parse.
    #[error("Invalid header name.")]
    InvalidHeaderName,

    /// A header value did not parse.
    #[error("Invalid header value.")]
    InvalidHeaderValue,

    /// The URL named a scheme other than `http` or `https`.
    #[error("Only http and https URLs are allowed.")]
    UnsupportedScheme,

    /// The URL carried a username or password.
    #[error("URLs with embedded credentials are not allowed.")]
    EmbeddedCredentials,

    /// The URL had no host component to check.
    #[error("URL must have a host.")]
    MissingHost,

    /// The URL named an address that must not be fetched.
    #[error("Private IP addresses are not allowed.")]
    PrivateAddress,

    /// The URL named a host that only resolves inside the deployment.
    #[error("Internal hostnames are not allowed.")]
    InternalHost,

    /// Every address the host resolved to must not be fetched.
    #[error("{host} resolves only to addresses that must not be fetched.")]
    #[non_exhaustive]
    UnfetchableResolution {
        /// The host that was resolved.
        host: String,
    },

    /// The redirect chain outran its bound.
    #[error("Too many redirects.")]
    TooManyRedirects,

    /// The response body exceeded the cap it was read under.
    #[error("The response body is larger than {limit} bytes.")]
    #[non_exhaustive]
    OversizedBody {
        /// The cap, in bytes.
        limit: usize,
    },

    /// The response body could not be read to completion.
    #[error("Could not read the response body.")]
    UnreadableBody(#[source] reqwest::Error),
}

impl Error {
    /// Every address `host` resolved to must not be fetched. `host` is the
    /// name alone, never the URL it came from.
    pub fn unfetchable_resolution(host: impl Into<String>) -> Self {
        Self::UnfetchableResolution { host: host.into() }
    }

    /// A response body outgrew `limit`, the most bytes it was read under.
    pub fn oversized_body(limit: usize) -> Self {
        Self::OversizedBody { limit }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(error: &dyn std::error::Error) -> String {
        let mut rendered = format!("{error} {error:?}");
        let mut source = error.source();
        while let Some(cause) = source {
            rendered.push_str(&format!(" {cause} {cause:?}"));
            source = cause.source();
        }
        rendered
    }

    #[tokio::test]
    async fn a_converted_transport_error_does_not_repeat_the_query_string() {
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a client builds")
            .get("http://127.0.0.1:1/search?key=hunter2")
            .send()
            .await
            .expect_err("nothing listens there");
        assert!(
            chain(&error).contains("hunter2"),
            "the probe carries no URL"
        );

        let error = Error::from(error);

        assert!(!chain(&error).contains("hunter2"), "{}", chain(&error));
    }

    #[test]
    fn each_constructor_fills_its_variant() {
        assert!(
            matches!(
                Error::unfetchable_resolution("inside.example"),
                Error::UnfetchableResolution { host } if host == "inside.example"
            ),
            "the host is kept"
        );
        assert!(
            matches!(
                Error::oversized_body(1_024),
                Error::OversizedBody { limit: 1_024 }
            ),
            "the limit is kept"
        );
    }
}
