use thiserror::Error;

/// Why a job's log could not be downloaded.
///
/// None of these carries the address the log was stored at, because GitHub
/// signs that address with a credential of its own.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JobLogError {
    /// GitHub redirected the request for the log without saying where to.
    #[error("GitHub redirected the job log without a destination")]
    Undirected,

    /// GitHub redirected the request for the log somewhere it is not sent
    /// on to: off its own origin to a host that does not store logs, over
    /// plain HTTP, or with credentials in the address.
    #[error("GitHub redirected the job log to an unexpected host")]
    UnexpectedHost,

    /// The log could not be reached where it is stored, or the time to
    /// download it ran out before the store answered. Time running out once
    /// the log has begun to arrive is [`JobLogError::Interrupted`].
    #[error("The job log download failed or timed out")]
    Unreachable,

    /// Where the log is stored answered with this HTTP status, not the log.
    #[error("The job log download returned HTTP {0}")]
    Refused(u16),

    /// The log stopped arriving part way through, or the time to download it
    /// ran out while it was arriving.
    #[error("The job log download failed part way through")]
    Interrupted,
}
