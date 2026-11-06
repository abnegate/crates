use std::iter::successors;
use thiserror::Error;

/// What separates one cause from the next in a [`ApplicationError::Transport`]
/// message.
const CAUSE_SEPARATOR: &str = ": ";

/// What went wrong authenticating as a GitHub App or asking GitHub about
/// one, worded so that no variant can carry a credential, the private key, a
/// manifest code, or the URL a request was sent to.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ApplicationError {
    /// The GitHub App API origin is not an HTTPS URL, or a plain HTTP one
    /// where plain HTTP was allowed, with nothing but a host and a path.
    #[error(
        "The GitHub App API origin is not an HTTPS URL, or a plain HTTP one where that was allowed, with no query, fragment or credentials."
    )]
    InvalidOrigin,

    /// The HTTP client GitHub App requests are sent with could not be built.
    #[error("The GitHub App request client could not be built.")]
    Client,

    /// The App's private key is not an RSA key in PEM form.
    #[error("The GitHub App private key could not be read.")]
    PrivateKey,

    /// The JWT could not be signed with the App's private key.
    #[error("The GitHub App JWT could not be signed.")]
    Signing,

    /// The GitHub App request did not complete, for the reason given and
    /// each cause beneath it, with the URL it was sent to removed.
    #[error("The GitHub App request failed: {0}")]
    Transport(String),

    /// GitHub answered the GitHub App request with this status, a redirect
    /// included, which is never followed.
    #[error("GitHub returned HTTP {0} for the GitHub App request.")]
    Status(u16),

    /// GitHub's answer to the GitHub App request could not be read. The
    /// answer is not repeated, since it may hold a token, a secret or a
    /// private key.
    #[error("GitHub's answer to the GitHub App request could not be read.")]
    Response,

    /// The manifest code is empty, `.` or `..`, so it cannot be sent: it
    /// would name no path segment, or the one before it.
    #[error("The GitHub App manifest code is empty or a dot segment, so it cannot be sent.")]
    InvalidCode,

    /// The GitHub App request was given an empty JWT or installation token,
    /// and was refused unsent, since GitHub answers none of them without one.
    #[error("The GitHub App request needs a JWT or token, and none was given.")]
    AuthenticationRequired,
}

impl ApplicationError {
    /// `error` as [`Self::Transport`], followed by each cause beneath it,
    /// without the URL `reqwest` appends to its message, which a redirect, a
    /// proxy or a later caller could put a credential in.
    pub(super) fn transport(error: reqwest::Error) -> Self {
        let error = error.without_url();
        let causes: Vec<String> = successors(
            Some(&error as &dyn std::error::Error),
            |cause: &&dyn std::error::Error| cause.source(),
        )
        .map(ToString::to_string)
        .collect();
        Self::Transport(causes.join(CAUSE_SEPARATOR))
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
            "GitHub returned HTTP 404 for the GitHub App request."
        );
        assert!(!ApplicationError::PrivateKey.to_string().contains("BEGIN"));
        assert!(!ApplicationError::Signing.to_string().contains("BEGIN"));
        assert!(!ApplicationError::Response.to_string().contains("BEGIN"));
    }

    #[test]
    fn an_invalid_code_does_not_echo_the_code() {
        let rendered = ApplicationError::InvalidCode.to_string();

        assert_eq!(
            rendered,
            "The GitHub App manifest code is empty or a dot segment, so it cannot be sent."
        );
        assert!(!rendered.contains("app-manifests"));
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

    #[tokio::test]
    async fn transport_errors_keep_their_causes_without_the_request_url() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let error = client
            .post("http://127.0.0.1:1/app/installations/1/access_tokens?credential=leaked")
            .send()
            .await
            .unwrap_err();

        let rendered = ApplicationError::transport(error).to_string();
        assert!(
            rendered.to_lowercase().contains("refused"),
            "the cause beneath reqwest's own message was dropped: {rendered}"
        );
        assert!(
            !rendered.contains("credential=leaked")
                && !rendered.contains("127.0.0.1")
                && !rendered.contains("access_tokens"),
            "the request URL survived into the error: {rendered}"
        );
    }
}
