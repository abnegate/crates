//! Naming a change, and opening, checking and merging a pull request for it,
//! through the public API.

use abnegate_vcs::git::GitService;
use abnegate_vcs::subject::Kind;
use abnegate_vcs::subject::Subject;

mod branch_names {
    use super::*;

    fn task() -> uuid::Uuid {
        uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    fn generate(title: &str) -> String {
        GitService::new()
            .generate_branch_name(task(), title)
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_branch_carries_the_task_and_a_slug_of_its_title() {
        assert_eq!(
            generate("Fix the login bug"),
            "task/12345678-fix-the-login-bug"
        );
    }

    #[test]
    fn nothing_a_branch_name_may_not_carry_survives_the_slug() {
        assert_eq!(
            generate("Add user@email validation!!!"),
            "task/12345678-add-user-email-validation"
        );
    }

    #[test]
    fn a_title_too_long_a_title_in_another_script_and_no_title_all_name_a_branch() {
        assert!(generate(&"A".repeat(200)).len() <= 100);

        for title in ["修复登录问题", "", "   "] {
            assert_eq!(generate(title), "task/12345678", "{title:?}");
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
    use abnegate_secret::SecretValue;
    use abnegate_vcs::BranchName;
    use abnegate_vcs::ChecksOutcome;
    use abnegate_vcs::CommitSha;
    use abnegate_vcs::Description;
    use abnegate_vcs::IssueComment;
    use abnegate_vcs::MergeMethod;
    use abnegate_vcs::Mergeability;
    use abnegate_vcs::MergeableState;
    use abnegate_vcs::MergedPullRequest;
    use abnegate_vcs::PullRequestDetail;
    use abnegate_vcs::PullRequestService;
    use abnegate_vcs::ReviewState;
    use abnegate_vcs::ReviewThreadRecord;
    use abnegate_vcs::SubmittedReview;
    use abnegate_vcs::ThreadComment;
    use abnegate_vcs::tally;
    use serde_json::json;
    use std::num::NonZeroU64;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    fn task() -> uuid::Uuid {
        uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    #[test]
    fn a_repository_url_in_any_form_github_uses_yields_its_owner_and_name() {
        let service = PullRequestService::new().unwrap();

        for url in [
            "https://github.com/acme-corp/my-project",
            "https://github.com/acme-corp/my-project.git",
            "git@github.com:acme-corp/my-project.git",
            "git@github.com:acme-corp/my-project",
            "ssh://git@github.com/acme-corp/my-project.git",
        ] {
            let repository = service.parse_github_url(url).expect(url);
            assert_eq!(repository.owner(), "acme-corp", "{url}");
            assert_eq!(repository.name(), "my-project", "{url}");
        }
    }

    /// The scheme was discarded before the host was checked, so an address
    /// nothing here can fetch or publish to still parsed as a repository this
    /// service answers for.
    #[test]
    fn an_address_this_service_does_not_speak_is_not_a_repository() {
        let service = PullRequestService::new().unwrap();

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
        let body = Description::new("The login form was not validating emails correctly", task())
            .with_report("Validated the address before submit, and covered it with a test.")
            .with_changes("- Modified `auth.rs`\n- Updated `login.html`")
            .with_url("https://tasks.example.com/tasks/12345678")
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
        let body = Description::new("Task description", task()).render();

        assert!(body.contains("12345678"), "{body}");
        assert!(!body.contains("this task]("), "{body}");
    }

    #[test]
    fn a_description_without_changes_heads_no_files_section() {
        let body = Description::new("Task description", task())
            .with_report("Nothing needed changing.")
            .with_url("https://tasks.example.com/tasks/123")
            .render();

        assert!(!body.contains("## Files"), "{body}");
    }

    /// Reviews a caller read somewhere else are tallied the same way as the
    /// ones this crate reads.
    #[test]
    fn reviews_a_caller_builds_are_tallied() {
        let tallied = tally(&[
            SubmittedReview::new(ReviewState::ChangesRequested, Some("ada".to_string())),
            SubmittedReview::new(ReviewState::Approved, Some("ada".to_string())),
            SubmittedReview::new(ReviewState::Approved, None),
        ]);

        assert_eq!(tallied.cycles, 1);
        assert_eq!(tallied.approvals, 2);
    }

    /// Each step takes what the one before it returned: the head the pull
    /// request was read at is the commit whose checks are read, and the head
    /// the merge insists on.
    #[tokio::test]
    async fn a_pull_request_is_driven_from_checks_to_merge_through_the_public_api() {
        let head = "a".repeat(40);
        let merged = "c".repeat(40);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "node_id": "PR_kwDOAcme7",
                "number": 7,
                "title": "(feat): basket totals",
                "body": "Adds totals to the basket.",
                "state": "open",
                "draft": false,
                "merged": false,
                "merge_commit_sha": null,
                "head": { "ref": "task/one", "sha": head },
                "base": { "ref": "main", "sha": "b".repeat(40) },
                "html_url": "https://github.com/acme/project/pull/7",
                "mergeable": true,
                "mergeable_state": "clean",
                "changed_files": 3,
                "additions": 120,
                "deletions": 14,
                "commits": 2,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/repos/acme/project/commits/{head}/check-runs"
            )))
            .and(query_param("filter", "latest"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 1,
                "check_runs": [{ "name": "build", "status": "completed", "conclusion": "success" }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/project/commits/{head}/status")))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 1,
                "statuses": [{ "context": "lint", "state": "success" }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/repos/acme/project/pulls/7/merge"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "commit_title": "(feat): basket totals (#7)",
                "commit_message": "Adds totals to the basket.",
                "sha": head,
                "merge_method": "squash",
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "sha": merged, "merged": true })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &server.uri()).unwrap();
        let reference = service
            .pull_request("https://github.com/acme/project/pull/7")
            .unwrap();
        let token = SecretValue::new("token");

        let pull = service.fetch_pull(&reference, &token).await.unwrap();

        assert_eq!(
            pull,
            PullRequestDetail::new(
                "PR_kwDOAcme7",
                NonZeroU64::new(7).unwrap(),
                "(feat): basket totals",
                BranchName::parse("task/one").unwrap(),
                CommitSha::parse(&head).unwrap(),
                BranchName::parse("main").unwrap(),
                "https://github.com/acme/project/pull/7",
            )
            .with_body("Adds totals to the basket.")
            .with_mergeable(Mergeability::Clean)
            .with_mergeable_state(MergeableState::Clean)
            .with_changed_files(3)
            .with_additions(120)
            .with_deletions(14)
            .with_commits(2)
        );

        let checks = service
            .fetch_checks(reference.repository(), &token, &pull.head_sha)
            .await
            .unwrap();

        assert_eq!(checks, ChecksOutcome::Success);
        assert_eq!(checks.label(), "success");

        let merge = service
            .merge(
                &reference,
                &token,
                Some(pull.node_id.as_str()),
                &pull.head_sha,
                &format!("{} (#{})", pull.title, pull.number),
                pull.body.as_deref().unwrap_or_default(),
                MergeMethod::Squash,
                false,
            )
            .await
            .unwrap();

        assert_eq!(
            merge,
            MergedPullRequest::new(Some(CommitSha::parse(&merged).unwrap()), false)
        );
    }

    /// A caller builds the comments and review threads it expects the way
    /// this crate reads them back, so it can compare them whole.
    #[tokio::test]
    async fn comments_and_review_threads_read_back_as_the_values_a_caller_builds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/7/comments"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": 11,
                "user": { "login": "review-bot" },
                "body": "Looks good once the empty cart is handled.",
                "html_url": "https://github.com/acme/project/pull/7#issuecomment-11",
                "created_at": "2026-09-20T09:00:00Z",
            }])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": { "reviewThreads": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [
                        {
                            "id": "PRRT_1",
                            "isResolved": true,
                            "isOutdated": false,
                            "path": "src/cart.ts",
                            "line": 12,
                            "comments": { "nodes": [{
                                "databaseId": 99,
                                "body": "handle the empty cart",
                                "url": "https://github.com/acme/project/pull/7#discussion_r99",
                                "createdAt": "2026-09-20T10:00:00Z",
                                "author": { "login": "review-bot" },
                            }] },
                        },
                        {
                            "id": "PRRT_2",
                            "isResolved": false,
                            "isOutdated": true,
                            "path": null,
                            "line": null,
                            "comments": { "nodes": [{
                                "databaseId": null,
                                "body": "why the second total?",
                                "url": "https://github.com/acme/project/pull/7#discussion_r100",
                                "createdAt": "2026-09-21T09:30:00Z",
                                "author": null,
                            }] },
                        },
                    ],
                } } } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &server.uri()).unwrap();
        let reference = service
            .pull_request("https://github.com/acme/project/pull/7")
            .unwrap();
        let token = SecretValue::new("token");

        let comments = service
            .fetch_issue_comments(&reference, &token)
            .await
            .unwrap();
        let threads = service
            .fetch_review_threads(&reference, &token)
            .await
            .unwrap();

        assert_eq!(
            comments,
            [IssueComment::new(
                11,
                "review-bot",
                "Looks good once the empty cart is handled.",
                "https://github.com/acme/project/pull/7#issuecomment-11",
                "2026-09-20T09:00:00Z",
            )]
        );
        assert_eq!(
            threads,
            [
                ReviewThreadRecord::new(
                    "PRRT_1",
                    vec![ThreadComment::new(
                        Some(99),
                        "review-bot",
                        "handle the empty cart",
                        "https://github.com/acme/project/pull/7#discussion_r99",
                        "2026-09-20T10:00:00Z",
                    )],
                )
                .with_resolved(true)
                .with_path("src/cart.ts")
                .with_line(12),
                ReviewThreadRecord::new(
                    "PRRT_2",
                    vec![ThreadComment::new(
                        None,
                        "",
                        "why the second total?",
                        "https://github.com/acme/project/pull/7#discussion_r100",
                        "2026-09-21T09:30:00Z",
                    )],
                )
                .with_outdated(true),
            ]
        );
    }
}
