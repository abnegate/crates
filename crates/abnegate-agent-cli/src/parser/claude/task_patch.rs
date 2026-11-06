use serde::Deserialize;

use crate::parser::lenient;

/// The fields of a task that a `task_updated` notice changed, of those a
/// [`Subagent`](crate::Subagent) carries.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TaskPatch {
    #[serde(deserialize_with = "lenient")]
    pub(crate) status: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) description: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) error: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) is_backgrounded: Option<bool>,
}
