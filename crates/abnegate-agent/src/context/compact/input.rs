use serde::Serialize;

use super::Source;

/// The user payload of a summary request.
#[derive(Serialize)]
pub(super) struct Input<'a> {
    pub(super) previous_state: &'a str,
    pub(super) sources: &'a [Source<'a>],
}
