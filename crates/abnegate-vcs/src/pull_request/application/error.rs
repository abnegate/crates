use thiserror::Error;

/// What went wrong authenticating as a GitHub App, worded so that no variant
/// can carry a credential, the private key, or the URL a request was sent to.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ApplicationError {
    /// The origin installation tokens were to be asked for is not an HTTPS
    /// URL with nothing but a host and a path.
    #[error(
        "The installation token origin is not an HTTPS URL with no query, fragment or credentials."
    )]
    InvalidOrigin,

    /// The HTTP client installation tokens are asked for with could not be
    /// built.
    #[error("The installation token client could not be built.")]
    Client,

    /// The App's private key is not an RSA key in PEM form.
    #[error("The GitHub App private key could not be read.")]
    PrivateKey,

    /// The JWT could not be signed with the App's private key.
    #[error("The GitHub App JWT could not be signed.")]
    Signing,

    /// The installation token request did not complete, for the reason
    /// given, with the URL it was sent to removed.
    #[error("The installation token request failed: {0}")]
    Transport(String),

    /// GitHub answered the installation token request with this status,
    /// a redirect included, which is never followed.
    #[error("GitHub returned HTTP {0} for the installation token request.")]
    Status(u16),

    /// GitHub's answer to the installation token request could not be read.
    /// The answer is not repeated, since it may hold the token.
    #[error("The installation token response could not be read.")]
    Response,
}

impl ApplicationError {
    /// `error` as [`Self::Transport`], without the URL `reqwest` appends to
    /// its message, which a redirect, a proxy or a later caller could put a
    /// credential in.
    pub(super) fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn no_variant_echoes_key_material() {
        assert_eq!(
            ApplicationError::Status(404).to_string(),
            "GitHub returned HTTP 404 for the installation token request."
        );
        assert!(!ApplicationError::PrivateKey.to_string().contains("BEGIN"));
        assert!(!ApplicationError::Signing.to_string().contains("BEGIN"));
    }

    #[tokio::test]
    async fn transport_errors_drop_the_request_url() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();

        let error = client
            .post("http://127.0.0.1:1/app/installations/1/access_tokens?credential=leaked")
            .send()
            .await
            .unwrap_err();

        let rendered = ApplicationError::transport(error).to_string();
        assert!(
            !rendered.contains("credential=leaked") && !rendered.contains("127.0.0.1"),
            "the request URL survived into the error: {rendered}"
        );
    }
}
