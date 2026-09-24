use uuid::Uuid;

/// What a pull request says, for a reviewer who was not in the chat.
///
/// The problem comes first because it is what the reviewer judges the change
/// against, then the run's own report of what it did about it, then the files
/// it touched. A run that reported nothing leaves its section out rather than
/// heading an empty one.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Description<'a> {
    /// What was wrong, as the task described it.
    pub problem: &'a str,
    /// The run's own account of what it did, if it gave one.
    pub report: Option<&'a str>,
    /// The files it changed, if they are listed.
    pub changes: Option<&'a str>,
    /// The task that opened the pull request.
    pub task: Uuid,
    /// Where the task can be read, if it can.
    pub url: Option<&'a str>,
}

impl<'a> Description<'a> {
    /// A description of the change `task` made to fix `problem`, with no
    /// report, no list of files and no link to the task.
    pub fn new(problem: &'a str, task: Uuid) -> Self {
        Self {
            problem,
            report: None,
            changes: None,
            task,
            url: None,
        }
    }

    /// The run's own account of what it did.
    #[must_use]
    pub fn with_report(mut self, report: &'a str) -> Self {
        self.report = Some(report);
        self
    }

    /// The files the change touched, already formatted as Markdown.
    #[must_use]
    pub fn with_changes(mut self, changes: &'a str) -> Self {
        self.changes = Some(changes);
        self
    }

    /// Where the task can be read, linked in place of its identifier.
    #[must_use]
    pub fn with_url(mut self, url: &'a str) -> Self {
        self.url = Some(url);
        self
    }

    /// The pull request body, in Markdown.
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
        let body = Description::new("The login form accepts an invalid email.", task())
            .with_report("Validated the address before submit. Added a regression test.")
            .with_changes("- `auth.rs`")
            .with_url("https://tasks.example.com/tasks/123")
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
        let problem = "Something was wrong.";
        for description in [
            Description::new(problem, task()),
            Description::new(problem, task()).with_report(""),
            Description::new(problem, task()).with_report("   \n "),
        ] {
            let body = description.render();

            assert!(!body.contains("## What changed"), "{body}");
            assert!(!body.contains("## Files"), "{body}");
            assert!(body.contains("## Problem"), "{body}");
        }
    }

    /// Without a console to link to, the id is what takes a reviewer back to
    /// the run that opened this.
    #[test]
    fn a_description_without_a_console_link_names_the_task_it_came_from() {
        let body = Description::new("Something was wrong.", task()).render();

        assert!(body.contains(&task().to_string()), "{body}");
        assert!(!body.contains("this task]("), "{body}");
    }
}
