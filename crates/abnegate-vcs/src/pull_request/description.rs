use uuid::Uuid;

/// What a pull request says, for a reviewer who was not in the chat.
///
/// The problem comes first because it is what the reviewer judges the change
/// against, then the run's own report of what it did about it, then the files
/// it touched. A run that reported nothing leaves its section out rather than
/// heading an empty one.
pub struct Description<'a> {
    pub problem: &'a str,
    pub report: Option<&'a str>,
    pub changes: Option<&'a str>,
    pub task: Uuid,
    pub url: Option<&'a str>,
}

impl Description<'_> {
    pub fn render(&self) -> String {
        let mut body = format!("## Problem\n\n{}\n\n", self.problem.trim());

        if let Some(report) = self.report.map(str::trim).filter(|it| !it.is_empty()) {
            body.push_str(&format!("## What changed\n\n{report}\n\n"));
        }

        if let Some(changes) = self.changes.map(str::trim).filter(|it| !it.is_empty()) {
            body.push_str(&format!("## Files\n\n{changes}\n\n"));
        }

        body.push_str("---\n");
        match self.url {
            Some(url) => body.push_str(&format!("Opened from [this task]({url}).\n")),
            None => body.push_str(&format!("Opened from task `{}`.\n", self.task)),
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    /// The reviewer was not in the chat, so what the change was for comes
    /// before what it did about it, and the run's own report is the account of
    /// the second.
    #[test]
    fn a_description_leads_with_the_problem_and_then_the_run_s_own_report() {
        let body = Description {
            problem: "The login form accepts an invalid email.",
            report: Some("Validated the address before submit. Added a regression test."),
            changes: Some("- `auth.rs`"),
            task: task(),
            url: Some("https://tasks.example.com/tasks/123"),
        }
        .render();

        let problem = body.find("## Problem").expect("{body}");
        let changed = body.find("## What changed").expect("{body}");
        let files = body.find("## Files").expect("{body}");

        assert!(problem < changed && changed < files, "{body}");
        assert!(body.contains("accepts an invalid email"), "{body}");
        assert!(body.contains("Added a regression test."), "{body}");
        assert!(body.contains("`auth.rs`"), "{body}");
        assert!(
            body.contains("[this task](https://tasks.example.com/tasks/123)"),
            "{body}"
        );
    }

    /// A heading over nothing reads as a section the reviewer has missed.
    #[test]
    fn a_run_that_reported_nothing_heads_no_empty_section() {
        for report in [None, Some(""), Some("   \n ")] {
            let body = Description {
                problem: "Something was wrong.",
                report,
                changes: None,
                task: task(),
                url: None,
            }
            .render();

            assert!(!body.contains("## What changed"), "{body}");
            assert!(!body.contains("## Files"), "{body}");
            assert!(body.contains("## Problem"), "{body}");
        }
    }

    /// Without a console to link to, the id is what takes a reviewer back to
    /// the run that opened this.
    #[test]
    fn a_description_without_a_console_link_names_the_task_it_came_from() {
        let body = Description {
            problem: "Something was wrong.",
            report: None,
            changes: None,
            task: task(),
            url: None,
        }
        .render();

        assert!(body.contains(&task().to_string()), "{body}");
        assert!(!body.contains("this task]("), "{body}");
    }
}
