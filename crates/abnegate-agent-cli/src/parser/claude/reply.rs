use serde::Deserialize;

use crate::parser::claude::cli_message::CliMessage;
use crate::parser::claude::report::Report;
use crate::parser::claude::report::account;
use crate::parser::lenient;

/// A `user` line, read straight from the stream rather than through
/// [`StreamEvent`](crate::StreamEvent), so that the tool's account of its
/// call is read only as far as a subagent's report goes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Reply {
    #[serde(deserialize_with = "lenient")]
    pub(crate) message: Option<CliMessage>,
    #[serde(alias = "toolUseResult", deserialize_with = "account")]
    pub(crate) tool_use_result: Option<Report>,
}
