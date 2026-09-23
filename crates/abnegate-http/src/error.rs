use thiserror::Error;

/// The result of every fallible operation in this crate.
pub type Result<T> = std::result::Result<T, HttpError>;

/// Everything that can go wrong issuing or validating an HTTP request.
#[derive(Debug, Error)]
pub enum HttpError {
    /// The underlying transport refused or failed the request.
    #[error(transparent)]
    Request(#[from] reqwest::Error),

    /// A response body did not deserialise into the requested type.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    /// The client implementation does not offer this method.
    #[error("{0} is not supported by this HTTP client")]
    Unsupported(reqwest::Method),
}
