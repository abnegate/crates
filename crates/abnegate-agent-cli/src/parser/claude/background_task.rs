use serde::Deserialize;

use crate::parser::lenient;

/// One entry of a `background_tasks_changed` notice: a task running in the
/// background.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BackgroundTask {
    #[serde(deserialize_with = "lenient")]
    pub(crate) task_id: Option<String>,
}
