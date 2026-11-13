/// The end of a job's log a read keeps when the log outgrows its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogEnd {
    /// The first bytes of the log, up to the limit.
    Start,
    /// The last bytes of the log, up to the limit.
    End,
}

impl LogEnd {
    /// The `Range` a store is asked for this end of a log by, if any: the
    /// last `limit` bytes for the end, and nothing for the start or for no
    /// bytes at all, which no range can ask for.
    pub(super) fn range(self, limit: usize) -> Option<String> {
        match (self, limit) {
            (Self::End, 1..) => Some(format!("bytes=-{limit}")),
            _ => None,
        }
    }
}
