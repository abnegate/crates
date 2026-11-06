use super::*;
use crate::commit_sha::CommitSha;
use crate::parse_error::ParseError;
use crate::pull_request::Branch;
use crate::pull_request::Page;
use crate::pull_request::github_commit::GitHubCommit;
use crate::pull_request::github_repository_branch::GitHubRepositoryBranch;
use crate::tag_name::TagName;

/// What GitHub says when the ref asked to be deleted is not there.
const ABSENT: &str = "Reference does not exist";

/// What GitHub says when no commit answers to the ref asked to be resolved.
const UNRESOLVED: &str = "No commit found";

/// The ref GitHub reads as the tip of a repository's default branch, which no
/// branch may be named.
const HEAD: &str = "HEAD";

/// Git's shorthand for `HEAD`, a revision expression rather than a name,
/// though git accepts it as a tag.
const CURRENT: &str = "@";

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
        let response = self
            .sent(self.request(Method::DELETE, url, token, ACCEPT)?)
            .await?;

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
    /// `reference` is held to git's rules for a tag name, which every branch
    /// name git accepts meets too, `HEAD` aside, and is
    /// [`PullRequestError::Parse`] with [`ParseError::BranchName`] before
    /// anything is sent when it breaks them, or when it is `@`, git's
    /// shorthand for `HEAD`.
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
        let named = reference == HEAD || (reference != CURRENT && !TagName::refuses(reference));
        if !named {
            return Err(ParseError::BranchName(reference.to_string()).into());
        }
        let url = self.repository_endpoint(repository, &["commits", reference])?;
        let response = self
            .sent(self.request(Method::GET, url, token, ACCEPT)?)
            .await?;

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

    /// Page `page` of `repository`'s branches, in the order GitHub lists
    /// them, at `per_page` rows a page up to GitHub's hundred.
    ///
    /// A row that leaves out anything GitHub sends on every branch, its name,
    /// the commit at its tip or whether it is protected, is
    /// [`PullRequestError::GitHubApi`], never a page without it. A name
    /// `git check-ref-format` refuses, such as `a..b`, or a tip named by
    /// anything but a full commit, is [`PullRequestError::Parse`] for the
    /// whole page. A name only a command line would misread, such as
    /// `+release` or `-dash`, is listed as GitHub spells it, a
    /// [`crate::RemoteBranchName`] that
    /// [`crate::RemoteBranchName::to_branch_name`] refuses.
    pub async fn list_branches(
        &self,
        repository: &Repository,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Branch>> {
        let asked = asked(per_page);
        let url = self.repository_endpoint(repository, &["branches"])?;
        let rows: Vec<GitHubRepositoryBranch> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let branches = rows
            .into_iter()
            .map(GitHubRepositoryBranch::branch)
            .collect::<Result<_, _>>()?;
        Ok(Page::after(branches, page, read, asked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::Expected;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use crate::remote_branch_name::RemoteBranchName;
    use serde_json::Value;
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
        for (reference, digit) in [("main", 'a'), ("v1.2.3", 'b'), ("HEAD", 'c'), ("+v1", 'd')] {
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

    /// Where acme/project's branches are listed.
    const BRANCHES: &str = "/repos/acme/project/branches";

    /// What GitHub sends on every branch, without which a row is unreadable.
    const REQUIRED: &[&str] = &["name", "commit", "protected"];

    /// A marker no error may repeat from an answer it refuses.
    const SENTINEL: &str = "RAW-SENTINEL";

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    fn tip(name: &str, digit: char, protected: bool) -> Branch {
        Branch::new(
            RemoteBranchName::parse(name).unwrap(),
            commit(digit),
            protected,
        )
    }

    fn described(branch: &Branch) -> Value {
        json!({
            "name": branch.name.as_str(),
            "commit": {
                "sha": branch.commit.as_str(),
                "url": format!("https://api.github.com/repos/acme/project/commits/{}", branch.commit),
            },
            "protected": branch.protected,
        })
    }

    fn row(name: &str, sha: &str) -> Value {
        json!({ "name": name, "commit": { "sha": sha }, "protected": false })
    }

    /// A page of branches padded to exactly `length` bytes with
    /// [`SENTINEL`].
    fn padded(length: usize) -> String {
        let opening = format!(r#"["{SENTINEL}"#);
        let closing = r#""]"#;
        let padding = "x".repeat(length - opening.len() - closing.len());
        format!("{opening}{padding}{closing}")
    }

    async fn listing(server: &MockServer, asked: &str, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(BRANCHES))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(query_is(&[("per_page", asked), ("page", "2")]))
            .respond_with(response)
            .expect(1)
            .mount(server)
            .await;
    }

    async fn listed(server: &MockServer, asked: &str, listed: Vec<Value>) {
        listing(
            server,
            asked,
            ResponseTemplate::new(200).set_body_json(listed),
        )
        .await;
    }

    async fn list(server: &MockServer, per_page: u8) -> PullRequestResult<Page<Branch>> {
        let service = stand_in(server).await;
        service
            .list_branches(&project(&service), &token(), page(2), rows(per_page))
            .await
    }

    #[tokio::test]
    async fn a_full_page_of_branches_is_asked_for_and_followed() {
        let server = MockServer::start().await;
        let branches = vec![tip("main", 'a', true), tip("develop", 'b', false)];
        listed(&server, "2", branches.iter().map(described).collect()).await;

        let page_read = list(&server, 2).await.unwrap();

        assert_eq!(page_read, Page::new(branches, Some(page(3))));
    }

    #[tokio::test]
    async fn a_short_page_of_branches_is_the_last() {
        let server = MockServer::start().await;
        let branches = vec![tip("main", 'a', true)];
        listed(&server, "2", branches.iter().map(described).collect()).await;

        let page_read = list(&server, 2).await.unwrap();

        assert_eq!(page_read, Page::new(branches, None));
    }

    #[tokio::test]
    async fn no_more_branches_are_asked_for_than_github_sends() {
        let server = MockServer::start().await;
        listed(&server, "100", Vec::new()).await;

        let page_read = list(&server, u8::MAX).await.unwrap();

        assert_eq!(page_read, Page::new(Vec::new(), None));
    }

    #[tokio::test]
    async fn a_slashed_branch_name_is_read_intact() {
        let server = MockServer::start().await;
        let slashed = tip("feature/one", 'c', false);
        listed(&server, "2", vec![described(&slashed)]).await;

        let page_read = list(&server, 2).await.unwrap();

        assert_eq!(page_read.items, [slashed]);
        assert_eq!(page_read.items[0].name.as_str(), "feature/one");
    }

    #[tokio::test]
    async fn a_branch_name_a_command_line_would_misread_is_listed_with_the_rest() {
        let server = MockServer::start().await;
        let branches = vec![
            tip("main", 'a', true),
            tip("+release", 'b', false),
            tip("-dash", 'c', false),
        ];
        listed(&server, "3", branches.iter().map(described).collect()).await;

        let page_read = list(&server, 3).await.unwrap();

        assert_eq!(page_read, Page::new(branches, Some(page(3))));
        for (branch, name) in page_read.items[1..].iter().zip(["+release", "-dash"]) {
            assert_eq!(branch.name.as_str(), name);
            assert_eq!(
                branch.name.to_branch_name(),
                Err(ParseError::BranchName(name.to_string())),
                "{name:?} reached a command line"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_name_git_refuses_fails_the_whole_page() {
        for name in ["", "a..b", "a b", "dir/.hidden", "branch.lock"] {
            let server = MockServer::start().await;
            listed(
                &server,
                "2",
                vec![
                    described(&tip("main", 'a', true)),
                    row(name, &"b".repeat(40)),
                ],
            )
            .await;

            let failure = list(&server, 2).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::BranchName(_))),
                "{name:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_tip_named_by_anything_but_a_full_commit_fails_the_whole_page() {
        for sha in ["abc1234", "", &"g".repeat(40)] {
            let server = MockServer::start().await;
            listed(&server, "2", vec![row("main", sha)]).await;

            let failure = list(&server, 2).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::CommitSha(_))),
                "{sha:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_row_missing_what_github_always_sends_fails_the_whole_page() {
        for required in REQUIRED {
            let server = MockServer::start().await;
            let mut partial = described(&tip("develop", 'b', false));
            partial.as_object_mut().unwrap().remove(*required);
            listed(
                &server,
                "2",
                vec![described(&tip("main", 'a', true)), partial],
            )
            .await;

            let failure = list(&server, 2).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{required} missing: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_branch_listing_is_reported_as_what_it_is() {
        let missing: Expected = |failure| matches!(failure, PullRequestError::NotFound);
        let unauthenticated: Expected =
            |failure| matches!(failure, PullRequestError::AuthenticationFailed);
        for (response, expected) in [
            (
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
                missing,
            ),
            (ResponseTemplate::new(401), unauthenticated),
        ] {
            let server = MockServer::start().await;
            listing(&server, "2", response).await;

            let failure = list(&server, 2).await.unwrap_err();

            assert!(expected(&failure), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_branch_page_past_the_answer_bound_is_an_error_and_never_a_shorter_list() {
        let server = MockServer::start().await;
        listing(
            &server,
            "2",
            ResponseTemplate::new(200).set_body_string(padded(MAXIMUM_ANSWER_BYTES + 1)),
        )
        .await;

        let failure = list(&server, 2).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == OVERSIZED),
            "{failure:?}"
        );
        assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
    }

    #[tokio::test]
    async fn a_branch_listing_redirected_to_another_origin_is_not_followed() {
        let origin = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        listing(
            &origin,
            "2",
            ResponseTemplate::new(301)
                .insert_header("location", format!("{}{BRANCHES}", elsewhere.uri())),
        )
        .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&elsewhere)
            .await;

        let result = list(&origin, 2).await;

        let received = elsewhere.received_requests().await.unwrap();
        assert!(
            received
                .iter()
                .all(|request| !request.headers.contains_key("authorization")),
            "the token must never reach another origin"
        );
        assert!(
            received.is_empty(),
            "a redirect to another origin must not be followed"
        );
        assert!(
            matches!(result, Err(PullRequestError::GitHubApi(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_branch_listing_that_stalls_is_given_up_on() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([]))
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;
        let service = PullRequestService::addressing(
            Origin::standing_in_for("github.com", &server.uri()).unwrap(),
            false,
            Duration::from_millis(200),
        )
        .unwrap();
        let started = std::time::Instant::now();

        let failure = service
            .list_branches(&project(&service), &token(), page(2), rows(2))
            .await;

        assert!(
            matches!(failure, Err(PullRequestError::Http(ref error)) if error.is_timeout()),
            "{failure:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
