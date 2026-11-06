use serde::Deserialize;

use crate::parser::claude::task_patch::TaskPatch;
use crate::parser::claude::task_usage::TaskUsage;
use crate::parser::lenient;

/// The `spawn_depth` of a task the main agent started itself.
const TOP_LEVEL_DEPTH: u64 = 1;

/// A `system` line, read straight from the stream rather than through
/// [`StreamEvent`](crate::StreamEvent): the notice that opens the session,
/// or news of a task claude runs beside the main agent, a subagent among
/// them. Every field is read leniently, so one in a new shape costs only
/// itself and never the session the line names.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Notice {
    #[serde(deserialize_with = "lenient")]
    pub(crate) subtype: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) session_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) task_id: Option<String>,
    /// The call that started the task.
    #[serde(deserialize_with = "lenient")]
    pub(crate) tool_use_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) description: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) task_type: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) is_backgrounded: Option<bool>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) subagent_type: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) usage: Option<TaskUsage>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) last_tool_name: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) summary: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) status: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub(crate) patch: Option<TaskPatch>,
    /// Whether a subagent, rather than the main agent, started the task.
    #[serde(deserialize_with = "lenient")]
    pub(crate) owned_by_subagent: Option<bool>,
    /// How deep the task is nested: [`TOP_LEVEL_DEPTH`] for one the main
    /// agent started, one more for each subagent between them.
    #[serde(deserialize_with = "lenient")]
    pub(crate) spawn_depth: Option<u64>,
}

impl Notice {
    /// Whether the task belongs to an agent a subagent started, which the
    /// main agent never did.
    pub(crate) fn nested(&self) -> bool {
        self.owned_by_subagent == Some(true)
            || self
                .spawn_depth
                .is_some_and(|depth| depth > TOP_LEVEL_DEPTH)
    }
}

#[cfg(test)]
mod tests {
    use super::Notice;

    fn notice(line: &str) -> Notice {
        serde_json::from_str(line).expect("a notice")
    }

    #[test]
    fn a_task_is_nested_when_claude_says_a_subagent_owns_it_or_it_sits_deeper() {
        for line in [
            r#"{"owned_by_subagent":true}"#,
            r#"{"spawn_depth":2}"#,
            r#"{"owned_by_subagent":false,"spawn_depth":3}"#,
        ] {
            assert!(notice(line).nested(), "{line}");
        }
        for line in [
            r#"{}"#,
            r#"{"owned_by_subagent":false,"spawn_depth":1}"#,
            r#"{"owned_by_subagent":"yes","spawn_depth":"deep"}"#,
        ] {
            assert!(!notice(line).nested(), "{line}");
        }
    }

    #[test]
    fn a_field_in_another_shape_costs_only_itself() {
        let notice = notice(
            r#"{"type":"system","subtype":"init","session_id":"6f1","status":42,"usage":"lots","patch":[]}"#,
        );

        assert_eq!(notice.subtype.as_deref(), Some("init"));
        assert_eq!(notice.session_id.as_deref(), Some("6f1"));
        assert!(notice.status.is_none());
        assert!(notice.usage.is_none());
        assert!(
            notice
                .patch
                .is_none_or(|patch| patch.status.is_none() && patch.description.is_none())
        );
    }
}
