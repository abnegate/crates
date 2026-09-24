use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::MergeMethod;
use crate::pull_request::MergedPullRequest;
use crate::pull_request::github_merge::GitHubMerge;
use crate::pull_request::graphql_error::GraphQlError;
use crate::pull_request::merge_request::MergeRequest;
use crate::pull_request::service::graphql::FORBIDDEN;
use crate::pull_request::service::graphql::NOT_FOUND;
use crate::pull_request::service::graphql::RATE_LIMITED;
use crate::pull_request::service::graphql::graphql_messages;
use crate::pull_request::service::graphql::reported;
use serde_json::Value;
use serde_json::json;

/// Merges the pull request its `input` names as an administrator may, past
/// branch protection that refuses everyone else, and asks whether it merged.
const MERGE: &str = "mutation($input:MergePullRequestInput!){mergePullRequest(input:$input){pullRequest{merged mergeCommit{oid}}}}";

/// Where the administrator merge's answer says whether the pull request
/// merged.
const MERGED: &str = "/mergePullRequest/pullRequest/merged";

/// Where the administrator merge's answer names the commit it made.
const OID: &str = "/mergePullRequest/pullRequest/mergeCommit/oid";

/// What GitHub says of a pull request that no longer merges cleanly, which
/// needs its conflict repaired rather than an administrator's override.
const NOT_MERGEABLE: &str = "not mergeable";

/// What GitHub's REST API says of a merge refused because the head or the
/// base branch changed while it was being made, which a fresh read retries.
const MODIFIED: &str = "was modified";

/// What GitHub's GraphQL API says of a merge refused because the head moved
/// from the one it was asked to expect.
const HEAD_MODIFIED: &str = "Head branch was modified";

/// What a refused administrator merge adds to branch protection's reason.
const ADMINISTRATOR_REFUSED: &str = "administrator merge refused";

/// What joins branch protection's reason to the administrator merge's.
const SEPARATOR: &str = "; ";

impl PullRequestService {
    /// Merge a pull request, but only while its head is still `head`.
    ///
    /// The merge is asked for through GitHub's REST API, which honours branch
    /// protection. When protection refuses it and `administrator` allows, it
    /// is asked for again through the GraphQL mutation an administrator
    /// merges with, which succeeds for a token that may bypass the protection
    /// and refuses every other. `node` is the pull request's GraphQL node id;
    /// without one there is no second attempt. Both attempts carry `head`, so
    /// a head that moved since it was read is never merged.
    ///
    /// A pull request that no longer merges cleanly is
    /// [`PullRequestError::NotMergeable`] and is never retried as an
    /// administrator. A refusal by protection is
    /// [`PullRequestError::Protected`], carrying GitHub's reason and, when the
    /// administrator merge was refused too, that reason as well. A head that
    /// moved, or a branch GitHub says was modified while it merged, is
    /// [`PullRequestError::HeadMoved`], which a fresh read and a retry
    /// resolve. A merge that happened is
    /// never reported as a failure, even when the commit it made cannot be
    /// read. An administrator merge is reported only when GitHub's answer
    /// says the pull request merged; any other answer is
    /// [`PullRequestError::GitHubApi`].
    pub async fn merge(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        node: Option<&str>,
        head: &CommitSha,
        title: &str,
        message: &str,
        method: MergeMethod,
        administrator: bool,
    ) -> PullRequestResult<MergedPullRequest> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "pulls",
            &number,
            "merge",
        ]);
        let request = MergeRequest {
            commit_title: title,
            commit_message: message,
            sha: head.as_str(),
            merge_method: method,
        };

        let response = sent(self.request(Method::PUT, url, token, ACCEPT).json(&request)).await?;

        let status = response.status();
        if status.is_success() {
            return Ok(MergedPullRequest {
                sha: merge_commit(response).await,
                administrator: false,
            });
        }
        let refusal = explained(response).await?;
        let reason = reason(status, &refusal);
        match status {
            StatusCode::CONFLICT => Err(PullRequestError::HeadMoved),
            StatusCode::UNPROCESSABLE_ENTITY => Err(PullRequestError::NotMergeable(reason)),
            StatusCode::METHOD_NOT_ALLOWED if refusal.mentions(MODIFIED) => {
                Err(PullRequestError::HeadMoved)
            }
            StatusCode::METHOD_NOT_ALLOWED if refusal.mentions(NOT_MERGEABLE) => {
                Err(PullRequestError::NotMergeable(reason))
            }
            StatusCode::METHOD_NOT_ALLOWED => match node.filter(|_| administrator) {
                Some(node) => administrator_merge(self, node, token, &request, &reason).await,
                None => Err(PullRequestError::Protected(reason)),
            },
            _ => Err(unexpected(status, &refusal)),
        }
    }
}

/// The merge `request` asks for, made through the mutation an administrator
/// merges with, after branch protection refused it for `reason`.
///
/// Only a refusal GitHub's GraphQL API reports is protection's; an exchange
/// that failed on its way there or back is reported as itself, and an answer
/// that does not say the pull request merged is not read as a merge.
async fn administrator_merge(
    service: &PullRequestService,
    node: &str,
    token: &SecretValue,
    request: &MergeRequest<'_>,
    reason: &str,
) -> PullRequestResult<MergedPullRequest> {
    let variables = json!({
        "input": {
            "pullRequestId": node,
            "mergeMethod": request.merge_method.graphql(),
            "commitHeadline": request.commit_title,
            "commitBody": request.commit_message,
            "expectedHeadOid": request.sha,
        },
    });
    match service
        .graphql_answer::<Value, _>(token, MERGE, &variables)
        .await?
    {
        Ok(data) if data.pointer(MERGED).and_then(Value::as_bool) == Some(true) => {
            Ok(MergedPullRequest {
                sha: data
                    .pointer(OID)
                    .and_then(Value::as_str)
                    .and_then(|oid| CommitSha::parse(oid).ok()),
                administrator: true,
            })
        }
        Ok(_) => Err(PullRequestError::GitHubApi(UNREADABLE.to_string())),
        Err(errors) => Err(administrator_refusal(reason, &errors)),
    }
}

/// The commit a completed merge made, when its answer names one this crate
/// can read.
async fn merge_commit(response: Response) -> Option<CommitSha> {
    let merge: GitHubMerge = decode(response).await.ok()?;
    CommitSha::parse(&merge.sha?).ok()
}

/// What the errors an administrator merge was refused with mean: a spent rate
/// limit is one, a head that moved is another, and anything else is
/// protection's refusal.
fn administrator_refusal(reason: &str, errors: &[GraphQlError]) -> PullRequestError {
    if reported(errors, RATE_LIMITED) {
        return PullRequestError::RateLimited;
    }
    let moved = HEAD_MODIFIED.to_ascii_lowercase();
    if errors
        .iter()
        .any(|error| error.message.to_ascii_lowercase().contains(&moved))
    {
        return PullRequestError::HeadMoved;
    }
    if reported(errors, NOT_FOUND) || reported(errors, FORBIDDEN) {
        return protected(reason, "");
    }
    protected(reason, &graphql_messages(errors))
}

/// Branch protection's `reason`, then the administrator merge's refusal and
/// GitHub's `messages` about it, which are left out when there are none.
fn protected(reason: &str, messages: &str) -> PullRequestError {
    let refused = match messages.is_empty() {
        true => ADMINISTRATOR_REFUSED.to_string(),
        false => format!("{ADMINISTRATOR_REFUSED}: {messages}"),
    };
    PullRequestError::Protected(format!("{reason}{SEPARATOR}{refused}"))
}

/// GitHub's words about a merge it refused with `status`, or that status
/// when it gave none.
fn reason(status: StatusCode, refusal: &GitHubRefusal) -> String {
    let summary = refusal.summary();
    match summary.is_empty() {
        true => returned(status),
        false => summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::body_partial_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// Where pull request 7's merge is asked for.
    const MERGE_PATH: &str = "/repos/acme/project/pulls/7/merge";

    /// Where the administrator merge is asked for.
    const GRAPHQL_PATH: &str = "/graphql";

    /// The GraphQL node id of pull request 7.
    const NODE: &str = "PR_node";

    /// What branch protection says of a merge that lacks an approval.
    const APPROVAL: &str =
        "At least 1 approving review is required by reviewers with write access.";

    fn refusing(status: u16, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_json(json!({ "message": message }))
    }

    fn merged_as(oid: &str) -> ResponseTemplate {
        carrying(json!({
            "mergePullRequest": { "pullRequest": { "merged": true, "mergeCommit": { "oid": oid } } },
        }))
    }

    fn carrying(data: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }

    async fn answering(
        rest: ResponseTemplate,
        graphql: ResponseTemplate,
        calls: u64,
    ) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(MERGE_PATH))
            .respond_with(rest)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(GRAPHQL_PATH))
            .respond_with(graphql)
            .expect(calls)
            .mount(&server)
            .await;
        server
    }

    async fn attempt(
        server: &MockServer,
        node: Option<&str>,
        administrator: bool,
    ) -> PullRequestResult<MergedPullRequest> {
        let service = stand_in(server).await;
        service
            .merge(
                &seven(&service),
                &token(),
                node,
                &commit('a'),
                "Title",
                "Body",
                MergeMethod::Squash,
                administrator,
            )
            .await
    }

    #[tokio::test]
    async fn a_merge_answered_by_a_redirect_elsewhere_is_not_a_merge() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(MERGE_PATH))
            .respond_with(
                ResponseTemplate::new(303)
                    .insert_header("location", format!("{}/elsewhere", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(GRAPHQL_PATH))
            .respond_with(merged_as(commit('d').as_str()))
            .expect(0)
            .mount(&server)
            .await;

        let failure = attempt(&server, Some(NODE), true)
            .await
            .expect_err("a GET answered after the merge was redirected merged nothing");

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == REDIRECTED),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_conflicted_pull_request_is_not_merged_as_an_administrator() {
        let server = answering(
            refusing(405, "Pull Request is not mergeable"),
            merged_as(commit('d').as_str()),
            0,
        )
        .await;

        let error = attempt(&server, Some(NODE), true)
            .await
            .expect_err("a conflict is not merged");

        assert_eq!(
            format!("{error:?}"),
            r#"NotMergeable("Pull Request is not mergeable")"#
        );
    }

    #[tokio::test]
    async fn a_protected_merge_is_retried_through_the_administrator_mutation() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(MERGE_PATH))
            .respond_with(refusing(405, APPROVAL))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(GRAPHQL_PATH))
            .and(body_partial_json(json!({ "variables": { "input": {
                "pullRequestId": NODE,
                "mergeMethod": "SQUASH",
                "expectedHeadOid": commit('a').as_str(),
            } } })))
            .respond_with(merged_as(commit('d').as_str()))
            .expect(1)
            .mount(&server)
            .await;

        let merged = attempt(&server, Some(NODE), true).await.unwrap();
        assert_eq!(
            merged,
            MergedPullRequest {
                sha: Some(commit('d')),
                administrator: true,
            }
        );

        let refused = attempt(&server, Some(NODE), false).await.unwrap_err();
        assert!(
            matches!(&refused, PullRequestError::Protected(reason) if reason == APPROVAL),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_administrator_merge_carries_both_reasons_and_a_moved_head_is_named() {
        let server = answering(
            refusing(405, "Required status check \"test\" is expected."),
            ResponseTemplate::new(200).set_body_json(json!({
                "data": null,
                "errors": [{ "message": "Base branch was modified. Review and try the merge again." }],
            })),
            1,
        )
        .await;

        match attempt(&server, Some(NODE), true).await.unwrap_err() {
            PullRequestError::Protected(reason) => assert_eq!(
                reason,
                "Required status check \"test\" is expected.; administrator merge refused: \
                 Base branch was modified. Review and try the merge again."
            ),
            other => panic!("{other:?}"),
        }

        let moved = answering(
            refusing(
                409,
                "Head branch was modified. Review and try the merge again.",
            ),
            merged_as(commit('d').as_str()),
            0,
        )
        .await;
        for node in [None, Some(NODE)] {
            assert!(
                matches!(
                    attempt(&moved, node, true).await,
                    Err(PullRequestError::HeadMoved)
                ),
                "{node:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_merge_refused_because_a_branch_was_modified_is_a_moved_head() {
        for message in [
            "Base branch was modified. Review and try the merge again.",
            "Head branch was modified. Review and try the merge again.",
        ] {
            let server =
                answering(refusing(405, message), merged_as(commit('d').as_str()), 0).await;

            let failure = attempt(&server, Some(NODE), true).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::HeadMoved),
                "{message}: {failure:?}"
            );
        }

        let server = answering(
            refusing(405, APPROVAL),
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "mergePullRequest": null },
                "errors": [{ "message": "Head branch was modified. Review and try the merge again." }],
            })),
            1,
        )
        .await;

        let failure = attempt(&server, Some(NODE), true).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::HeadMoved),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_merge_is_asked_for_with_the_head_it_expects_and_the_method_github_names() {
        for (chosen, rest, graphql) in [
            (MergeMethod::Squash, "squash", "SQUASH"),
            (MergeMethod::Merge, "merge", "MERGE"),
            (MergeMethod::Rebase, "rebase", "REBASE"),
        ] {
            let head = commit('a');
            let asked = json!({
                "commit_title": "Title",
                "commit_message": "Body",
                "sha": head.as_str(),
                "merge_method": rest,
            });

            let open = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path(MERGE_PATH))
                .and(header("authorization", "Bearer token"))
                .and(header("accept", ACCEPT))
                .and(body_json(&asked))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "sha": commit('b').as_str(),
                    "merged": true,
                    "message": "Pull Request successfully merged",
                })))
                .expect(1)
                .mount(&open)
                .await;
            let service = stand_in(&open).await;
            let merged = service
                .merge(
                    &seven(&service),
                    &token(),
                    Some(NODE),
                    &head,
                    "Title",
                    "Body",
                    chosen,
                    true,
                )
                .await
                .unwrap();
            assert_eq!(
                merged,
                MergedPullRequest {
                    sha: Some(commit('b')),
                    administrator: false,
                },
                "{rest}"
            );

            let protected = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path(MERGE_PATH))
                .and(body_json(&asked))
                .respond_with(refusing(405, APPROVAL))
                .expect(1)
                .mount(&protected)
                .await;
            Mock::given(method("POST"))
                .and(path(GRAPHQL_PATH))
                .and(header("authorization", "Bearer token"))
                .and(body_json(json!({
                    "query": MERGE,
                    "variables": { "input": {
                        "pullRequestId": NODE,
                        "mergeMethod": graphql,
                        "commitHeadline": "Title",
                        "commitBody": "Body",
                        "expectedHeadOid": head.as_str(),
                    } },
                })))
                .respond_with(merged_as(commit('c').as_str()))
                .expect(1)
                .mount(&protected)
                .await;
            let service = stand_in(&protected).await;
            let merged = service
                .merge(
                    &seven(&service),
                    &token(),
                    Some(NODE),
                    &head,
                    "Title",
                    "Body",
                    chosen,
                    true,
                )
                .await
                .unwrap();
            assert_eq!(
                merged,
                MergedPullRequest {
                    sha: Some(commit('c')),
                    administrator: true,
                },
                "{graphql}"
            );
        }
    }

    #[tokio::test]
    async fn a_merge_github_refuses_for_want_of_access_or_rate_is_reported_as_that() {
        for (response, expected) in [
            (refusing(401, "Bad credentials"), "AuthenticationFailed"),
            (
                refusing(403, "Resource not accessible by integration"),
                "Forbidden",
            ),
            (
                refusing(403, "API rate limit exceeded")
                    .insert_header("x-ratelimit-remaining", "0"),
                "RateLimited",
            ),
            (
                refusing(403, "You have exceeded a secondary rate limit")
                    .insert_header("retry-after", "60"),
                "RateLimited",
            ),
            (
                refusing(403, "You have exceeded a secondary rate limit"),
                "RateLimited",
            ),
            (refusing(429, "Too many requests"), "RateLimited"),
            (refusing(404, "Not Found"), "NotFound"),
            (
                ResponseTemplate::new(422).set_body_json(json!({
                    "message": "Validation Failed",
                    "errors": [{ "message": "Merge method squash is not allowed" }],
                })),
                r#"NotMergeable("Validation Failed; Merge method squash is not allowed")"#,
            ),
        ] {
            let server = answering(response, merged_as(commit('d').as_str()), 0).await;

            let failure = attempt(&server, Some(NODE), true).await.unwrap_err();

            assert_eq!(format!("{failure:?}"), expected);
        }
    }

    #[tokio::test]
    async fn an_administrator_merge_without_a_node_is_not_attempted() {
        let server = answering(refusing(405, APPROVAL), merged_as(commit('d').as_str()), 0).await;

        for (node, administrator) in [(None, true), (Some(NODE), false), (None, false)] {
            let refused = attempt(&server, node, administrator).await.unwrap_err();

            assert_eq!(
                format!("{refused:?}"),
                format!("Protected({APPROVAL:?})"),
                "{node:?}, {administrator}"
            );
        }
    }

    #[tokio::test]
    async fn an_administrator_merge_the_token_may_not_make_is_refused_by_protection() {
        for kind in ["FORBIDDEN", "NOT_FOUND"] {
            let server = answering(
                refusing(405, APPROVAL),
                ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "mergePullRequest": null },
                    "errors": [{ "type": kind, "message": "Viewer may not merge" }],
                })),
                1,
            )
            .await;

            let refused = attempt(&server, Some(NODE), true).await.unwrap_err();

            assert_eq!(
                format!("{refused:?}"),
                format!(
                    "Protected({:?})",
                    format!("{APPROVAL}; administrator merge refused")
                ),
                "{kind}"
            );
        }
    }

    #[tokio::test]
    async fn a_merge_refusal_carries_github_s_message_and_not_its_body() {
        let sentinel = SecretValue::new("TOKEN-SENTINEL");
        let noisy = |status: u16| {
            ResponseTemplate::new(status).set_body_json(json!({
                "message": "Changes must be made through a pull request.",
                "documentation_url": "https://docs.github.com/RAW-SENTINEL",
                "status": "RAW-SENTINEL",
            }))
        };
        let silent =
            |status: u16| ResponseTemplate::new(status).set_body_string("<p>RAW-SENTINEL</p>");
        let graphql = ResponseTemplate::new(200).set_body_json(json!({
            "data": { "mergePullRequest": "RAW-SENTINEL" },
            "errors": [{
                "message": "Base branch was modified",
                "path": ["RAW-SENTINEL"],
                "extensions": { "raw": "RAW-SENTINEL" },
            }],
        }));

        for (rest, administrator, expected) in [
            (
                noisy(405),
                false,
                r#"Protected("Changes must be made through a pull request.")"#,
            ),
            (
                noisy(405),
                true,
                r#"Protected("Changes must be made through a pull request.; administrator merge refused: Base branch was modified")"#,
            ),
            (
                silent(405),
                false,
                r#"Protected("GitHub API returned 405 Method Not Allowed")"#,
            ),
            (
                silent(405),
                true,
                r#"Protected("GitHub API returned 405 Method Not Allowed; administrator merge refused: Base branch was modified")"#,
            ),
            (
                noisy(422),
                true,
                r#"NotMergeable("Changes must be made through a pull request.")"#,
            ),
            (
                noisy(500),
                true,
                r#"GitHubApi("GitHub API returned 500 Internal Server Error: Changes must be made through a pull request.")"#,
            ),
            (
                silent(502),
                true,
                r#"GitHubApi("GitHub API returned 502 Bad Gateway")"#,
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path(MERGE_PATH))
                .respond_with(rest)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path(GRAPHQL_PATH))
                .respond_with(graphql.clone())
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .merge(
                    &seven(&service),
                    &sentinel,
                    Some(NODE),
                    &commit('a'),
                    "Title",
                    "Body",
                    MergeMethod::Squash,
                    administrator,
                )
                .await
                .unwrap_err();

            assert_eq!(format!("{failure:?}"), expected);
            assert!(
                !format!("{failure} {failure:?}").contains("SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_merge_refusal_github_gives_no_reason_for_is_named_by_its_status() {
        for (response, administrator, expected) in [
            (
                ResponseTemplate::new(405),
                false,
                r#"Protected("GitHub API returned 405 Method Not Allowed")"#,
            ),
            (
                ResponseTemplate::new(405).set_body_json(json!({ "message": " \n " })),
                false,
                r#"Protected("GitHub API returned 405 Method Not Allowed")"#,
            ),
            (
                ResponseTemplate::new(422),
                true,
                r#"NotMergeable("GitHub API returned 422 Unprocessable Entity")"#,
            ),
            (
                ResponseTemplate::new(422).set_body_json(json!({ "errors": [] })),
                true,
                r#"NotMergeable("GitHub API returned 422 Unprocessable Entity")"#,
            ),
        ] {
            let server = answering(response, merged_as(commit('d').as_str()), 0).await;

            let failure = attempt(&server, Some(NODE), administrator)
                .await
                .unwrap_err();

            assert_eq!(format!("{failure:?}"), expected);
        }
    }

    #[tokio::test]
    async fn a_completed_merge_without_a_readable_commit_is_still_a_merge() {
        for answer in [
            ResponseTemplate::new(200).set_body_string("Pull Request successfully merged"),
            ResponseTemplate::new(200).set_body_json(json!({ "merged": true })),
            ResponseTemplate::new(200).set_body_json(json!({ "sha": null })),
            ResponseTemplate::new(200).set_body_json(json!({ "sha": 7 })),
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "not-a-commit" })),
            ResponseTemplate::new(204),
        ] {
            let server = answering(answer, merged_as(commit('d').as_str()), 0).await;

            assert_eq!(
                attempt(&server, Some(NODE), true).await.unwrap(),
                MergedPullRequest {
                    sha: None,
                    administrator: false,
                }
            );
        }

        for data in [
            json!({ "mergePullRequest": { "pullRequest": { "merged": true } } }),
            json!({ "mergePullRequest": { "pullRequest": { "merged": true, "mergeCommit": null } } }),
            json!({ "mergePullRequest": { "pullRequest": {
                "merged": true,
                "mergeCommit": { "oid": "not-a-commit" },
            } } }),
        ] {
            let server = answering(refusing(405, APPROVAL), carrying(data.clone()), 1).await;

            assert_eq!(
                attempt(&server, Some(NODE), true).await.unwrap(),
                MergedPullRequest {
                    sha: None,
                    administrator: true,
                },
                "{data}"
            );
        }
    }

    #[tokio::test]
    async fn an_administrator_merge_github_does_not_confirm_is_not_reported_as_one() {
        for data in [
            json!({}),
            json!({ "mergePullRequest": null }),
            json!({ "mergePullRequest": { "pullRequest": null } }),
            json!({ "mergePullRequest": { "pullRequest": { "merged": false, "mergeCommit": null } } }),
            json!({ "mergePullRequest": { "pullRequest": { "merged": "true" } } }),
            json!({ "mergePullRequest": { "pullRequest": {
                "mergeCommit": { "oid": commit('d').as_str() },
            } } }),
        ] {
            let server = answering(refusing(405, APPROVAL), carrying(data.clone()), 1).await;

            let failure = attempt(&server, Some(NODE), true)
                .await
                .expect_err("an answer that does not say the pull request merged is not a merge");

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{data}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_administrator_merge_that_fails_in_transit_is_not_called_protected() {
        for (graphql, expected) in [
            (
                refusing(500, "Server Error"),
                r#"GitHubApi("GitHub API returned 500 Internal Server Error: Server Error")"#
                    .to_string(),
            ),
            (
                ResponseTemplate::new(502).set_body_string("<p>Bad gateway</p>"),
                r#"GitHubApi("GitHub API returned 502 Bad Gateway")"#.to_string(),
            ),
            (
                ResponseTemplate::new(200).set_body_string("not an answer"),
                format!("GitHubApi({UNREADABLE:?})"),
            ),
            (
                ResponseTemplate::new(401),
                "AuthenticationFailed".to_string(),
            ),
            (
                refusing(403, "Resource not accessible by integration"),
                "Forbidden".to_string(),
            ),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                "RateLimited".to_string(),
            ),
            (ResponseTemplate::new(404), "NotFound".to_string()),
            (
                ResponseTemplate::new(200).set_body_json(json!({
                    "data": null,
                    "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }],
                })),
                "RateLimited".to_string(),
            ),
        ] {
            let server = answering(refusing(405, APPROVAL), graphql, 1).await;

            let failure = attempt(&server, Some(NODE), true).await.unwrap_err();

            assert_eq!(format!("{failure:?}"), expected);
        }

        let stalled = answering(
            refusing(405, APPROVAL),
            merged_as(commit('d').as_str()).set_delay(Duration::from_secs(5)),
            1,
        )
        .await;
        let impatient = PullRequestService {
            client: client(false, Duration::from_millis(250)).unwrap(),
            origin: Origin::standing_in_for("github.com", &stalled.uri()).unwrap(),
        };

        let failure = impatient
            .merge(
                &seven(&impatient),
                &token(),
                Some(NODE),
                &commit('a'),
                "Title",
                "Body",
                MergeMethod::Squash,
                true,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(&failure, PullRequestError::Http(error) if error.is_timeout()),
            "{failure:?}"
        );
    }
}
