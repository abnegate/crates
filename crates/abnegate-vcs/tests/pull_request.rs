//! Naming a change and opening a pull request for it, through the public API.

use abnegate_vcs::git::GitService;
use abnegate_vcs::subject::Kind;
use abnegate_vcs::subject::Subject;

mod branch_names {
    use super::*;

    fn task() -> uuid::Uuid {
        uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    #[test]
    fn a_branch_carries_the_task_and_a_slug_of_its_title() {
        let branch = GitService::new().generate_branch_name(task(), "Fix the login bug");

        assert!(branch.starts_with("task/12345678-"), "{branch}");
        assert!(branch.contains("fix"), "{branch}");
        assert!(branch.contains("login"), "{branch}");
        assert!(branch.contains("bug"), "{branch}");
        assert_eq!(branch, branch.to_lowercase());
    }

    #[test]
    fn nothing_a_branch_name_may_not_carry_survives_the_slug() {
        let branch = GitService::new().generate_branch_name(task(), "Add user@email validation!!!");

        assert!(!branch.contains('@'), "{branch}");
        assert!(!branch.contains('!'), "{branch}");
        assert!(!branch.contains("--"), "{branch}");
    }

    #[test]
    fn a_title_too_long_a_title_in_another_script_and_no_title_all_name_a_branch() {
        let service = GitService::new();

        assert!(service.generate_branch_name(task(), &"A".repeat(200)).len() <= 100);

        let other_script = service.generate_branch_name(task(), "修复登录问题");
        assert!(other_script.is_ascii(), "{other_script}");
        assert!(other_script.starts_with("task/12345678-"), "{other_script}");

        for title in ["", "   "] {
            let branch = service.generate_branch_name(task(), title);
            assert!(branch.starts_with("task/12345678-"), "{branch}");
            assert!(!branch.contains(' '), "{branch}");
        }
    }
}

/// The title is the change's own subject in the format this history uses, so a
/// reviewer reads it in a list of pull requests the same way they read a list
/// of commits.
#[test]
fn a_pull_request_is_titled_with_a_conventional_commit_subject() {
    let subject = Subject::new(Kind::Fix, "Validate the email before submit.");

    assert_eq!(
        subject.to_string(),
        "(fix): validate the email before submit"
    );
}

#[cfg(feature = "github")]
mod pull_requests {
    use abnegate_vcs::pull_request::Description;
    use abnegate_vcs::pull_request::PrService;

    fn task() -> uuid::Uuid {
        uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    #[test]
    fn a_repository_url_in_any_form_github_uses_yields_its_owner_and_name() {
        let service = PrService::new();

        for url in [
            "https://github.com/acme-corp/my-project",
            "https://github.com/acme-corp/my-project.git",
            "git@github.com:acme-corp/my-project.git",
            "git@github.com:acme-corp/my-project",
            "ssh://git@github.com/acme-corp/my-project.git",
        ] {
            assert_eq!(
                service.parse_github_url(url).expect(url),
                ("acme-corp".to_string(), "my-project".to_string()),
                "{url}"
            );
        }
    }

    /// The scheme was discarded before the host was checked, so an address
    /// nothing here can fetch or publish to still parsed as a repository this
    /// service answers for.
    #[test]
    fn an_address_this_service_does_not_speak_is_not_a_repository() {
        let service = PrService::new();

        for url in [
            "ftp://github.com/owner/repo",
            "http://github.com/owner/repo",
            "file://github.com/owner/repo",
            "javascript://github.com/owner/repo",
            "https://gitlab.com/owner/repo",
            "not-a-github-url",
        ] {
            assert!(
                service.parse_github_url(url).is_err(),
                "{url} is not an address a repository is published through"
            );
        }
    }

    #[test]
    fn a_description_carries_the_problem_the_report_and_the_files() {
        let body = Description {
            problem: "The login form was not validating emails correctly",
            report: Some("Validated the address before submit, and covered it with a test."),
            changes: Some("- Modified `auth.rs`\n- Updated `login.html`"),
            task: task(),
            url: Some("https://tasks.example.com/tasks/12345678"),
        }
        .render();

        assert!(body.contains("## Problem"), "{body}");
        assert!(body.contains("not validating emails"), "{body}");
        assert!(body.contains("## What changed"), "{body}");
        assert!(body.contains("covered it with a test"), "{body}");
        assert!(body.contains("## Files"), "{body}");
        assert!(body.contains("auth.rs"), "{body}");
        assert!(body.contains("tasks.example.com"), "{body}");
    }

    #[test]
    fn a_description_without_a_console_link_names_the_task() {
        let body = Description {
            problem: "Task description",
            report: None,
            changes: None,
            task: task(),
            url: None,
        }
        .render();

        assert!(body.contains("12345678"), "{body}");
        assert!(!body.contains("this task]("), "{body}");
    }

    #[test]
    fn a_description_without_changes_heads_no_files_section() {
        let body = Description {
            problem: "Task description",
            report: Some("Nothing needed changing."),
            changes: None,
            task: task(),
            url: Some("https://tasks.example.com/tasks/123"),
        }
        .render();

        assert!(!body.contains("## Files"), "{body}");
    }
}
