use thiserror::Error;

use crate::login::claude::Refusal;

/// Why a sign-in step failed.
///
/// No variant carries a credential, a response body or anything the request
/// sent: a refusal is its status and the [`Refusal`] its OAuth `error` code
/// names, and a transport failure has every value the request carried
/// scrubbed from it.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::login::LoginError;
///
/// fn status(error: &LoginError) -> Option<u16> {
///     match error {
///         LoginError::Rejected { status, refusal: _ } => Some(*status),
///         _ => None,
///     }
/// }
/// # let _ = status;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoginError {
    /// A value a sign-in step was handed, or a grant the token endpoint sent,
    /// is not one it can use. The message says what was expected, and quotes
    /// none of it.
    #[error("{0}")]
    Malformed(&'static str),
    /// An endpoint the sign-in would reach was refused: `reason` says why,
    /// without the URL.
    #[error("the {name} endpoint was refused: {reason}")]
    #[non_exhaustive]
    Endpoint {
        /// Which endpoint, such as `token`.
        name: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// The token endpoint answered with an HTTP status other than success,
    /// redirects included, since none is followed.
    #[error("the sign-in service refused the request with HTTP {status}")]
    #[non_exhaustive]
    Rejected {
        /// The HTTP status it answered with.
        status: u16,
        /// What its OAuth `error` code names, when it sent one.
        refusal: Option<Refusal>,
    },
    /// The sign-in service could not be reached, or its answer could not be
    /// read to the end.
    #[error("could not reach the sign-in service: {0}")]
    Transport(String),
    /// The operating system could not supply the random bytes a PKCE
    /// verifier and state are drawn from.
    #[error("could not draw random bytes for the sign-in")]
    Entropy,
}
