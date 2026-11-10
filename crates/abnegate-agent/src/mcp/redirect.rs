use std::error::Error;
use std::fmt;

/// Same-origin redirects a remote server's request follows before the next
/// is refused.
pub(super) const MAXIMUM_REDIRECTS: usize = 10;

/// A redirect a remote server's request was refused, carried as the source
/// of the request's own error, so its failure says why without a stale
/// record of some other request's redirect.
#[derive(Debug)]
pub(super) enum Redirect {
    /// One that left the origin first requested or changed the method; it
    /// holds where it pointed, as its origin and path.
    Refused(String),
    /// One past [`MAXIMUM_REDIRECTS`].
    Exceeded,
}

impl fmt::Display for Redirect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(location) => write!(formatter, "refused a redirect to {location}"),
            Self::Exceeded => write!(formatter, "refused more than {MAXIMUM_REDIRECTS} redirects"),
        }
    }
}

impl Error for Redirect {}
