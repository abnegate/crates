use serde::Deserialize;

use crate::parser::lenient;

/// The input of a call that starts a subagent.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Delegation {
    #[serde(deserialize_with = "lenient")]
    pub(crate) description: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) subagent_type: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) model: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) run_in_background: Option<bool>,
}
