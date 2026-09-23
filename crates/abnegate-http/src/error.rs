use thiserror::Error;

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, HttpError>;

/// Everything that can go wrong issuing or validating an HTTP request.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpError {
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
    UnfetchableResolution {
        /// The host that was resolved.
        host: String,
    },

    /// The redirect chain outran its bound.
    #[error("Too many redirects.")]
    TooManyRedirects,

    /// The response body exceeded the cap it was read under.
    #[error("The response body is larger than {limit} bytes.")]
    OversizedBody {
        /// The cap, in bytes.
        limit: usize,
    },

    /// The response body could not be read to completion.
    #[error("Could not read the response body.")]
    UnreadableBody(#[source] reqwest::Error),
}

impl From<reqwest::Error> for HttpError {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn chain(error: &dyn Error) -> String {
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

        let error = HttpError::from(error);

        assert!(!chain(&error).contains("hunter2"), "{}", chain(&error));
    }
}
