/// The start or the end of a job's log, read up to a byte limit, and whether
/// the log held more than that.
///
/// The bytes are the log as GitHub stores it. Its start, from
/// [`crate::PullRequestService::fetch_job_log`], is cut at the limit
/// wherever it falls, even inside a character. Its end, from
/// [`crate::PullRequestService::fetch_job_log_tail`], begins at the first
/// UTF-8 character after the cut, so it may be up to three bytes short of the
/// limit. A caller decodes and excerpts it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobLog {
    /// What was read of the log.
    pub content: Vec<u8>,
    /// Whether the log held more than was read: it went on past its start,
    /// or began before its end.
    pub truncated: bool,
}

impl JobLog {
    /// A log read as `content`, which `truncated` says the log held more
    /// than.
    pub fn new(content: Vec<u8>, truncated: bool) -> Self {
        Self { content, truncated }
    }
}
