use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;

use crate::login::claude::Refusal;

/// Why a sign-in step failed.
///
/// No variant carries a credential, a response body or anything the request
/// sent: a refusal is its status and the [`Refusal`] its OAuth `error` code
/// names, and a transport failure has every value the request carried
/// scrubbed from it. A CLI's own words have its one-time code, every secret
/// it was given and anything shaped like a credential redacted from them.
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
    ///
    /// Its message names the status alone, and never repeats what the
    /// service said: word what the person signing in is shown from `status`
    /// and `refusal`.
    #[error("the sign-in service refused the request with HTTP {status}")]
    #[non_exhaustive]
    Rejected {
        /// The HTTP status it answered with.
        status: u16,
        /// What its OAuth `error` code names. `None` means the refusal is not
        /// one this recognises: the answer carried no OAuth `error` code, or
        /// could not be read, such as a redirect, an outage page or an error
        /// shaped some other way.
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
    /// The CLI could not be started, such as when it is not installed.
    #[error("{} could not be started: {reason}", executable.display())]
    #[non_exhaustive]
    Unavailable {
        /// The CLI that was run.
        executable: PathBuf,
        /// Why the operating system could not start it.
        reason: String,
    },
    /// The CLI printed something other than what the step reads, or did not
    /// finish in time.
    #[error("{reason}")]
    #[non_exhaustive]
    Unreadable {
        /// What was missing or wrong.
        reason: String,
    },
    /// The CLI failed: `reason` is its own words, or how it exited when it
    /// gave none.
    #[error("{reason}")]
    #[non_exhaustive]
    Failed {
        /// Why it failed.
        reason: String,
    },
    /// The one-time code expired before anyone entered it, and the CLI was
    /// stopped.
    #[error("the sign-in code expired before anyone entered it")]
    #[non_exhaustive]
    Expired {
        /// When the code expired.
        expires_at: DateTime<Utc>,
    },
    /// The caller cancelled the sign-in, and the CLI was stopped.
    #[error("the sign-in was cancelled")]
    Cancelled,
    /// A file or directory the sign-in prepares or saves could not be. It
    /// names a path on this host, so it is for logs rather than the person
    /// signing in.
    #[error("could not {action} {}: {reason}", path.display())]
    #[non_exhaustive]
    Filesystem {
        /// What was being done, such as `prepare`.
        action: &'static str,
        /// The path it was done to.
        path: PathBuf,
        /// Why it could not be.
        reason: String,
    },
}
