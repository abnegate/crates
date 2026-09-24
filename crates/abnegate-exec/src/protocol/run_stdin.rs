use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Writes to a running command's stdin.
///
/// `Debug` prints the length of [`data`](Self::data) and never the data,
/// which can carry a credential.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct RunStdin {
    /// The job whose stdin is written
    pub job_id: String,
    /// The bytes to write, base64 encoded
    pub data: String,
    /// Close stdin once `data` is written
    #[serde(default)]
    pub eof: bool,
}

impl RunStdin {
    /// Write `data`, already base64 encoded, to `job_id`'s stdin and leave it
    /// open.
    pub fn new(job_id: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            data: data.into(),
            eof: false,
        }
    }

    /// Whether to close stdin once the data is written.
    pub fn with_eof(mut self, eof: bool) -> Self {
        self.eof = eof;
        self
    }
}

impl fmt::Debug for RunStdin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunStdin")
            .field("job_id", &self.job_id)
            .field("data", &format_args!("<{} bytes>", self.data.len()))
            .field("eof", &self.eof)
            .finish()
    }
}
