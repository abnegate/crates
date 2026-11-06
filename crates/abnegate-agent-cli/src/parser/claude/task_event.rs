/// The `system` notices claude tells of a task it runs beside the main
/// agent with, a subagent among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskEvent {
    Started,
    Progress,
    Updated,
    Notification,
}

impl TaskEvent {
    const ALL: &'static [Self] = &[
        Self::Started,
        Self::Progress,
        Self::Updated,
        Self::Notification,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "task_started",
            Self::Progress => "task_progress",
            Self::Updated => "task_updated",
            Self::Notification => "task_notification",
        }
    }

    /// The task event a notice of `subtype` is, if any.
    pub(crate) fn named(subtype: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|event| event.as_str() == subtype)
    }
}

#[cfg(test)]
mod tests {
    use super::TaskEvent;

    #[test]
    fn each_task_event_is_named_by_its_subtype_and_nothing_else_is_one() {
        for &event in TaskEvent::ALL {
            assert_eq!(TaskEvent::named(event.as_str()), Some(event));
        }
        for subtype in ["init", "task_invented", "background_tasks_changed", ""] {
            assert_eq!(TaskEvent::named(subtype), None, "{subtype}");
        }
    }
}
