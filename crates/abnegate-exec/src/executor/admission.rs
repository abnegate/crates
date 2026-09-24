/// How much of a chunk of output an [`OutputLimiter`](super::OutputLimiter)
/// lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Admission {
    /// Bytes of the chunk to deliver, from its start
    pub accepted: usize,
    /// Whether this is the first chunk to lose bytes to the limit, and so the
    /// one to warn about
    pub first_truncation: bool,
}
