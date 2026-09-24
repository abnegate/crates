use crate::pull_request::ThreadComment;

/// One review thread on a pull request's diff, with every comment in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThreadRecord {
    /// GitHub's node identifier for the thread, which resolving it takes.
    pub id: String,
    /// Whether someone marked it resolved.
    pub resolved: bool,
    /// Whether the lines it was left on have changed since.
    pub outdated: bool,
    /// The file it was left on, if GitHub says.
    pub path: Option<String>,
    /// The line it was left on, if that line is still in the diff.
    pub line: Option<u32>,
    /// Its comments, oldest first.
    pub comments: Vec<ThreadComment>,
}
