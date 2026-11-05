/// The start of a job's log, read up to a byte limit, and whether the log
/// went on past it.
///
/// The bytes are the log as GitHub stores it, cut at the limit wherever it
/// falls, even inside a character: a caller decodes and excerpts it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobLog {
    /// What was read of the log.
    pub content: Vec<u8>,
    /// Whether the log went on past the limit.
    pub truncated: bool,
}

impl JobLog {
    /// A log read as `content`, which `truncated` says the log went on past.
    pub fn new(content: Vec<u8>, truncated: bool) -> Self {
        Self { content, truncated }
    }
}
