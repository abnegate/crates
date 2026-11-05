use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::github_commit::GitHubCommit;

/// What GitHub says when the ref asked to be deleted is not there.
const ABSENT: &str = "Reference does not exist";

/// What GitHub says when no commit answers to the ref asked to be resolved.
const UNRESOLVED: &str = "No commit found";

/// The ref GitHub reads as the tip of a repository's default branch, which no
/// branch may be named.
const HEAD: &str = "HEAD";

impl PullRequestService {
    /// Delete a branch; one already gone is not an error.
    ///
    /// GitHub answers a branch that is gone with a 404, or with a 422 saying
    /// the reference does not exist. It answers a repository or branch the
    /// token cannot see with the same 404, so that also reads as already gone.
    /// Any other refusal, such as one to delete a protected or default branch,
    /// is reported.
    pub async fn delete_branch(
        &self,
        repository: &Repository,
        token: &SecretValue,
        branch: &BranchName,
    ) -> PullRequestResult<()> {
        let reference = branch.reference();
        let segments: Vec<&str> = ["git"].into_iter().chain(reference.split('/')).collect();
        let url = self.repository_endpoint(repository, &segments)?;
        let response = sent(self.request(Method::DELETE, url, token, ACCEPT)).await?;

        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let refusal = explained(response).await?;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(ABSENT) {
            return Ok(());
        }
        Err(unexpected(status, &refusal))
    }

    /// The commit `reference` names now: a branch, a tag, a commit given in
    /// full or in part, or `HEAD` for the tip of the default branch.
    ///
    /// `reference` is held to a branch name's rules, `HEAD` aside, and is
    /// [`PullRequestError::Parse`] before anything is sent when it breaks them.
    /// It travels as one path segment, so a `/` in it is sent as `%2F`. A
    /// reference GitHub holds no commit for, which it answers with a 404 or
    /// with a 422 saying no commit was found, is [`PullRequestError::NotFound`].
    /// An answer that names no commit is [`PullRequestError::GitHubApi`], and
    /// one that names a commit by anything but a full identifier is
    /// [`PullRequestError::Parse`].
    pub async fn resolve_commit(
        &self,
        repository: &Repository,
        token: &SecretValue,
        reference: &str,
    ) -> PullRequestResult<CommitSha> {
        if reference != HEAD {
            BranchName::parse(reference)?;
        }
        let url = self.repository_endpoint(repository, &["commits", reference])?;
        let response = sent(self.request(Method::GET, url, token, ACCEPT)).await?;

        let status = response.status();
        if status.is_success() {
            let commit: GitHubCommit = decode(response).await?;
            return Ok(CommitSha::parse(&commit.sha)?);
        }
        let refusal = explained(response).await?;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(UNRESOLVED) {
            return Err(PullRequestError::NotFound);
        }
        Err(unexpected(status, &refusal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::Expected;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::json;
    use std::mem::discriminant;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    const HEADS: &str = "/repos/acme/project/git/refs/heads";

    const COMMITS: &str = "/repos/acme/project/commits";

    async fn resolve(server: &MockServer, reference: &str) -> PullRequestResult<CommitSha> {
        let service = stand_in(server).await;
        service
            .resolve_commit(&project(&service), &token(), reference)
            .await
    }

    async fn resolving(sent: &str, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{COMMITS}/{sent}")))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    fn named(digit: char) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "sha": digit.to_string().repeat(40),
            "node_id": "C_kwDOAAAAAA",
            "commit": { "message": "Ship it" },
            "parents": [],
        }))
    }

    async fn delete(server: &MockServer, name: &str) -> PullRequestResult<()> {
        let branch = BranchName::parse(name)?;
        let service = stand_in(server).await;
        service
            .delete_branch(&project(&service), &token(), &branch)
            .await
    }

    async fn answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("{HEADS}/task/one")))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_branch_is_deleted_by_its_ref_one_encoded_segment_at_a_time() {
        for (name, sent) in [
            ("task/one", "task/one"),
            ("feature#x", "feature%23x"),
            ("fix/%2e%2e", "fix/%252e%252e"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .and(path(format!("{HEADS}/{sent}")))
                .and(header("authorization", "Bearer token"))
                .and(header("accept", ACCEPT))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;

            delete(&server, name).await.expect(name);
        }
    }

    #[tokio::test]
    async fn a_branch_already_gone_is_not_an_error() {
        for response in [
            ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            ResponseTemplate::new(404),
            ResponseTemplate::new(422).set_body_json(json!({ "message": ABSENT })),
            ResponseTemplate::new(422).set_body_json(json!({
                "message": "Validation Failed",
                "errors": ["reference does not exist"],
            })),
        ] {
            let server = answering(response).await;

            delete(&server, "task/one").await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_branch_github_refuses_to_delete_is_reported() {
        for (response, expected) in [
            (
                ResponseTemplate::new(422).set_body_json(json!({
                    "message": "Cannot delete this protected branch",
                    "documentation_url": "RAW-SENTINEL",
                })),
                "GitHub API returned 422 Unprocessable Entity: Cannot delete this protected branch",
            ),
            (
                ResponseTemplate::new(422)
                    .set_body_string("<html>Reference does not exist RAW-SENTINEL</html>"),
                "GitHub API returned 422 Unprocessable Entity",
            ),
            (
                ResponseTemplate::new(500),
                "GitHub API returned 500 Internal Server Error",
            ),
        ] {
            let server = answering(response).await;

            let failure = delete(&server, "task/one").await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == expected),
                "{failure:?}"
            );
            assert!(
                !format!("{failure:?}").contains("RAW-SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_name_that_could_change_the_request_target_cannot_be_named() {
        let server = MockServer::start().await;

        for name in [
            "a//b",
            "trailing/",
            "/leading",
            "a/../b",
            "..",
            "a/./b",
            "a?b=c",
            "a\tb",
            ".\t.",
            "a/.\t./b",
            "a\nb",
        ] {
            let failure = delete(&server, name).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::BranchName(_))),
                "{name:?}: {failure:?}"
            );
        }
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "nothing reached GitHub"
        );
    }

    #[tokio::test]
    async fn a_refused_deletion_is_reported_as_what_it_is() {
        for (response, expected) in [
            (
                ResponseTemplate::new(401),
                PullRequestError::AuthenticationFailed,
            ),
            (ResponseTemplate::new(403), PullRequestError::Forbidden),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                PullRequestError::RateLimited,
            ),
            (ResponseTemplate::new(429), PullRequestError::RateLimited),
        ] {
            let server = answering(response).await;

            let failure = delete(&server, "task/one").await.unwrap_err();

            assert_eq!(
                discriminant(&failure),
                discriminant(&expected),
                "{expected:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_or_tag_resolves_to_the_commit_it_names() {
        for (reference, digit) in [("main", 'a'), ("v1.2.3", 'b'), ("HEAD", 'c')] {
            let server = resolving(reference, named(digit)).await;

            let resolved = resolve(&server, reference).await.expect(reference);

            assert_eq!(resolved, commit(digit), "{reference}");
        }
    }

    #[tokio::test]
    async fn a_slashed_reference_travels_as_one_encoded_segment() {
        for (reference, sent) in [
            ("feature/one", "feature%2Fone"),
            ("heads/feature/one", "heads%2Ffeature%2Fone"),
            ("feature#x", "feature%23x"),
            ("fix/%2e%2e", "fix%2F%252e%252e"),
        ] {
            let server = resolving(sent, named('d')).await;

            let resolved = resolve(&server, reference).await.expect(reference);

            assert_eq!(resolved, commit('d'), "{reference}");
        }
    }

    #[tokio::test]
    async fn a_full_commit_resolves_to_itself() {
        let full = "e".repeat(40);
        let server = resolving(&full, named('e')).await;

        let resolved = resolve(&server, &full).await.unwrap();

        assert_eq!(resolved, commit('e'));
    }

    #[tokio::test]
    async fn a_reference_github_holds_no_commit_for_is_not_found() {
        for response in [
            ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            ResponseTemplate::new(422).set_body_json(json!({
                "message": "No commit found for SHA: nowhere",
                "documentation_url": "https://docs.github.com/rest/commits/commits#get-a-commit",
            })),
        ] {
            let server = resolving("nowhere", response).await;

            let failure = resolve(&server, "nowhere").await.unwrap_err();

            assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn any_other_refusal_to_resolve_is_reported_as_what_it_is() {
        let invalid: Expected = |failure| {
            matches!(failure, PullRequestError::GitHubApi(text)
                if text == "GitHub API returned 422 Unprocessable Entity: Validation Failed")
        };
        let empty: Expected = |failure| {
            matches!(failure, PullRequestError::GitHubApi(text)
                if text == "GitHub API returned 409 Conflict: Git Repository is empty.")
        };
        let unauthenticated: Expected =
            |failure| matches!(failure, PullRequestError::AuthenticationFailed);
        let forbidden: Expected = |failure| matches!(failure, PullRequestError::Forbidden);
        let limited: Expected = |failure| matches!(failure, PullRequestError::RateLimited);
        for (response, expected) in [
            (
                ResponseTemplate::new(422).set_body_json(json!({ "message": "Validation Failed" })),
                invalid,
            ),
            (
                ResponseTemplate::new(409).set_body_json(json!({
                    "message": "Git Repository is empty.",
                })),
                empty,
            ),
            (ResponseTemplate::new(401), unauthenticated),
            (ResponseTemplate::new(403), forbidden),
            (ResponseTemplate::new(429), limited),
        ] {
            let server = resolving("main", response).await;

            let failure = resolve(&server, "main").await.unwrap_err();

            assert!(expected(&failure), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_reference_that_could_change_the_request_target_is_refused_unsent() {
        let server = MockServer::start().await;

        for reference in [
            "",
            "..",
            ".",
            "a/../b",
            "a/./b",
            "a//b",
            "/leading",
            "trailing/",
            "a?b=c",
            "a b",
            " main",
            "a\tb",
            "a\nb",
            "-x",
            "@",
            "a@{1}",
            "a~1",
            "a^",
            "a.lock",
        ] {
            let failure = resolve(&server, reference).await;

            assert!(
                matches!(
                    failure,
                    Err(PullRequestError::Parse(ParseError::BranchName(_)))
                ),
                "{reference:?}: {failure:?}"
            );
        }
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "nothing reached GitHub"
        );
    }

    #[tokio::test]
    async fn an_answer_that_names_no_full_commit_is_an_error() {
        let unreadable: Expected =
            |failure| matches!(failure, PullRequestError::GitHubApi(text) if text == UNREADABLE);
        let malformed: Expected =
            |failure| matches!(failure, PullRequestError::Parse(ParseError::CommitSha(_)));
        for (answer, expected) in [
            (json!({}), unreadable),
            (json!({ "sha": null }), unreadable),
            (json!({ "sha": 7 }), unreadable),
            (json!([]), unreadable),
            (json!({ "sha": "" }), malformed),
            (json!({ "sha": "abc1234" }), malformed),
            (json!({ "sha": "g".repeat(40) }), malformed),
            (json!({ "sha": "a".repeat(41) }), malformed),
        ] {
            let server = resolving("main", ResponseTemplate::new(200).set_body_json(&answer)).await;

            let failure = resolve(&server, "main").await.unwrap_err();

            assert!(expected(&failure), "{answer}: {failure:?}");
        }
    }
}
