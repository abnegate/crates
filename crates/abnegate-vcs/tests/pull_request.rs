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

    #[test]
    fn a_caller_can_hang_the_task_off_its_own_prefix() {
        let service = GitService::new()
            .with_branch_prefix(abnegate_vcs::BranchName::parse("acme/task").unwrap())
            .with_branch_format(abnegate_vcs::BranchFormat::Joined);
        let task = uuid::Uuid::parse_str("1a2b3c4d-1234-1234-1234-123456789abc").unwrap();

        let branch = service.generate_branch_name(task, "slug").unwrap();

        assert_eq!(branch.to_string(), "acme/task-1a2b3c4d-slug");
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

#[cfg(all(feature = "github", feature = "testing"))]
mod pull_requests {
    use abnegate_secret::SecretValue;
    use abnegate_vcs::AutoMerge;
    use abnegate_vcs::BranchName;
    use abnegate_vcs::ChecksOutcome;
    use abnegate_vcs::CommitSha;
    use abnegate_vcs::Description;
    use abnegate_vcs::IssueComment;
    use abnegate_vcs::MergeMethod;
    use abnegate_vcs::MergeSettings;
    use abnegate_vcs::Mergeability;
    use abnegate_vcs::MergeableState;
    use abnegate_vcs::MergedPullRequest;
    use abnegate_vcs::PullRequestAutomation;
    use abnegate_vcs::PullRequestDetail;
    use abnegate_vcs::PullRequestError;
    use abnegate_vcs::PullRequestService;
    use abnegate_vcs::ReviewComment;
    use abnegate_vcs::ReviewRecord;
    use abnegate_vcs::ReviewState;
    use abnegate_vcs::ReviewThreadCount;
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
    use wiremock::matchers::body_partial_json;
    use wiremock::matchers::body_string_contains;
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

    #[tokio::test]
    async fn reviews_read_back_as_the_values_a_caller_builds() {
        let server = MockServer::start().await;
        let head = "39edddf0d1abc06b84ebb74e8213c1fd41ec333e";
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": 5,
                "user": { "login": "review-bot" },
                "body": "Score: 4/5",
                "state": "COMMENTED",
                "commit_id": head,
                "submitted_at": "2026-09-28T02:51:09Z",
            }])))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &server.uri()).unwrap();
        let reference = service
            .pull_request("https://github.com/acme/project/pull/7")
            .unwrap();

        let reviews = service
            .fetch_reviews(&reference, &SecretValue::new("token"))
            .await
            .unwrap();

        assert_eq!(
            reviews,
            [ReviewRecord::new(5, "review-bot", "Score: 4/5")
                .with_commit(CommitSha::parse(head).unwrap())
                .with_submitted_at("2026-09-28T02:51:09Z")]
        );
    }

    /// A caller counts a pull request's review threads without their
    /// comments, and compares the count with one it builds.
    #[tokio::test]
    async fn review_threads_count_as_the_value_a_caller_builds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": { "reviewThreads": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [
                        { "id": "PRRT_1", "isResolved": true },
                        { "id": "PRRT_2", "isResolved": false },
                        { "id": "PRRT_3", "isResolved": false },
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

        let count = service
            .fetch_review_thread_counts(&reference, &SecretValue::new("token"))
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::new(2, 1, true));
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

    /// A caller reads one review comment whole by the id a thread names, and
    /// builds the comment it expects the way this crate reads it back.
    #[tokio::test]
    async fn a_review_comment_reads_back_as_the_value_a_caller_builds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/comments/99"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 99,
                "user": { "login": "review-bot" },
                "body": "handle the empty cart",
                "path": "src/cart.ts",
                "html_url": "https://github.com/acme/project/pull/7#discussion_r99",
                "created_at": "2026-09-20T10:00:00Z",
                "pull_request_url": "https://api.github.com/repos/acme/project/pulls/7",
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &server.uri()).unwrap();
        let reference = service
            .pull_request("https://github.com/acme/project/pull/7")
            .unwrap();

        let comment = service
            .fetch_review_comment(&reference, &SecretValue::new("token"), 99)
            .await
            .unwrap();

        assert_eq!(
            comment,
            ReviewComment::new(
                99,
                "review-bot",
                "handle the empty cart",
                "https://github.com/acme/project/pull/7#discussion_r99",
                "2026-09-20T10:00:00Z",
            )
            .with_path("src/cart.ts")
        );
    }

    /// A draft is read, taken out of draft and set to merge once it is ready
    /// by the node id the read named, and a caller builds what it expects the
    /// way this crate reads it back.
    #[tokio::test]
    async fn a_draft_is_made_ready_and_set_to_merge_through_the_public_api() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("autoMergeAllowed"))
            .and(body_partial_json(
                json!({ "variables": { "owner": "acme", "name": "project", "number": 7 } }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": {
                    "autoMergeAllowed": true,
                    "squashMergeAllowed": true,
                    "mergeCommitAllowed": true,
                    "rebaseMergeAllowed": false,
                    "pullRequest": {
                        "id": "PR_node",
                        "isDraft": true,
                        "autoMergeRequest": null,
                        "viewerCanEnableAutoMerge": true,
                        "viewerCanDisableAutoMerge": false,
                    },
                } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("markPullRequestReadyForReview"))
            .and(body_json(json!({
                "query": "mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){clientMutationId}}",
                "variables": { "id": "PR_node" },
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "markPullRequestReadyForReview": { "clientMutationId": null } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains("enablePullRequestAutoMerge"))
            .and(body_partial_json(json!({ "variables": { "input": {
                "pullRequestId": "PR_node",
                "mergeMethod": "SQUASH",
            } } })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": null,
                "errors": [{ "message": "Pull request Pull request is in clean status" }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &server.uri()).unwrap();
        let reference = service
            .pull_request("https://github.com/acme/project/pull/7")
            .unwrap();
        let token = SecretValue::new("token");

        let automation = service.fetch_automation(&reference, &token).await.unwrap();
        assert_eq!(
            automation,
            PullRequestAutomation::new(
                "PR_node",
                MergeSettings::new(true, vec![MergeMethod::Squash, MergeMethod::Merge]),
            )
            .with_draft(true)
            .with_can_enable_auto_merge(true)
        );

        service
            .mark_ready_for_review(&automation.node_id, &token)
            .await
            .unwrap();
        let refused = service
            .enable_auto_merge(
                &automation.node_id,
                &token,
                &AutoMerge::new(MergeMethod::Squash),
            )
            .await
            .unwrap_err();

        assert!(
            matches!(refused, PullRequestError::AlreadyMergeable),
            "{refused:?}"
        );
    }
}
