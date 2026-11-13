use super::contents::DIFF;
use super::contents::excerpt;
use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::Commit;
use crate::pull_request::Excerpt;
use crate::pull_request::Page;
use crate::pull_request::github_listed_commit::GitHubListedCommit;

impl PullRequestService {
    /// Page `page` of the commits on a pull request's branch, oldest first,
    /// at `per_page` rows a page up to GitHub's hundred.
    ///
    /// GitHub lists at most 250 commits for a pull request, so a longer
    /// branch is listed only that far. A row that leaves out the commit's
    /// identifier, link or message is [`PullRequestError::GitHubApi`], and one
    /// whose identifier is not a full commit is [`PullRequestError::Parse`],
    /// never a page without it. A commit whose author no GitHub account
    /// claims is listed with no [`Commit::author_login`].
    pub async fn list_commits(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Commit>> {
        let asked = asked(per_page);
        let number = reference.number().to_string();
        let url =
            self.repository_endpoint(reference.repository(), &["pulls", &number, "commits"])?;
        let rows: Vec<GitHubListedCommit> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let commits = rows
            .into_iter()
            .map(GitHubListedCommit::commit)
            .collect::<PullRequestResult<Vec<Commit>>>()?;
        Ok(Page::after(commits, page, read, asked))
    }

    /// The start of one commit's unified diff against its first parent, read
    /// no further than `limit` bytes.
    ///
    /// The excerpt is never longer than `limit` bytes, never ends inside a
    /// character, and says whether the diff went on past what it holds. A
    /// commit the repository does not hold is [`PullRequestError::NotFound`].
    pub async fn fetch_commit_diff(
        &self,
        repository: &Repository,
        token: &SecretValue,
        sha: &CommitSha,
        limit: usize,
    ) -> PullRequestResult<Excerpt> {
        let url = self.repository_endpoint(repository, &["commits", sha.as_str()])?;

        let response = self
            .answered(self.request(Method::GET, url, token, DIFF)?)
            .await?;
        excerpt(response, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::enterprise;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    const COMMITS: &str = "/repos/acme/project/pulls/7/commits";

    const AUTHORED: &str = "2026-11-06T09:30:00Z";

    /// What GitHub sends on every commit, without which a row is unreadable.
    const REQUIRED: &[&str] = &["sha", "html_url", "commit"];

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    fn link(digit: char) -> String {
        format!(
            "https://github.com/acme/project/commit/{}",
            commit(digit).as_str()
        )
    }

    fn row(digit: char) -> Value {
        json!({
            "sha": commit(digit).as_str(),
            "html_url": link(digit),
            "commit": {
                "message": format!("feat: change {digit}\n\nWhy it changed."),
                "author": { "name": "Mona Lisa", "email": "mona@example.com", "date": AUTHORED },
            },
            "author": { "login": "octocat" },
        })
    }

    fn written(digit: char) -> Commit {
        Commit::new(
            commit(digit),
            format!("feat: change {digit}\n\nWhy it changed."),
            link(digit),
        )
        .with_author_name("Mona Lisa")
        .with_author_login("octocat")
        .with_authored_at(AUTHORED)
    }

    async fn listing(server: &MockServer, route: &str, asked: &str, listed: Vec<Value>) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(query_is(&[("per_page", asked), ("page", "2")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn list(service: &PullRequestService, per_page: u8) -> PullRequestResult<Page<Commit>> {
        service
            .list_commits(&seven(service), &token(), page(2), rows(per_page))
            .await
    }

    #[tokio::test]
    async fn a_full_page_of_commits_is_read_whole_and_followed() {
        let server = MockServer::start().await;
        listing(&server, COMMITS, "2", vec![row('a'), row('b')]).await;
        let service = stand_in(&server).await;

        let listed = list(&service, 2).await.unwrap();

        assert_eq!(
            listed,
            Page::new(vec![written('a'), written('b')], Some(page(3)))
        );
    }

    #[tokio::test]
    async fn a_short_page_of_commits_is_the_last_and_never_asks_past_a_hundred() {
        let server = MockServer::start().await;
        listing(&server, COMMITS, "100", vec![row('a')]).await;
        let service = stand_in(&server).await;

        let listed = list(&service, u8::MAX).await.unwrap();

        assert_eq!(listed, Page::new(vec![written('a')], None));
    }

    #[tokio::test]
    async fn a_commit_no_account_claims_is_listed_without_a_login() {
        let mut unclaimed = row('a');
        unclaimed["author"] = Value::Null;
        let mut unsigned = row('b');
        unsigned["commit"].as_object_mut().unwrap().remove("author");
        let server = MockServer::start().await;
        listing(&server, COMMITS, "5", vec![unclaimed, unsigned]).await;
        let service = stand_in(&server).await;

        let listed = list(&service, 5).await.unwrap();

        let mut anonymous = written('a');
        anonymous.author_login = None;
        let undated = Commit::new(commit('b'), "feat: change b\n\nWhy it changed.", link('b'))
            .with_author_login("octocat");
        assert_eq!(listed.items, [anonymous, undated]);
    }

    #[tokio::test]
    async fn a_commit_row_missing_what_github_always_sends_fails_the_whole_page() {
        for required in REQUIRED {
            let mut broken = row('b');
            broken.as_object_mut().unwrap().remove(*required);
            let server = MockServer::start().await;
            listing(&server, COMMITS, "5", vec![row('a'), broken]).await;
            let service = stand_in(&server).await;

            let failure = list(&service, 5).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{required} missing: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_commit_named_by_anything_but_a_full_identifier_fails_the_page() {
        let mut abbreviated = row('b');
        abbreviated["sha"] = json!("abc123");
        let server = MockServer::start().await;
        listing(&server, COMMITS, "5", vec![row('a'), abbreviated]).await;
        let service = stand_in(&server).await;

        let failure = list(&service, 5).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::CommitSha(ref value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn commits_of_a_pull_request_that_cannot_be_seen_are_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(COMMITS))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = list(&service, 5).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn commits_are_listed_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        listing(&server, &format!("/api/v3{COMMITS}"), "5", vec![row('a')]).await;
        let service = enterprise(&server).await;

        let listed = list(&service, 5).await.unwrap();

        assert_eq!(listed.items, [written('a')]);
    }

    fn diff_path(prefix: &str) -> String {
        format!(
            "{prefix}/repos/acme/project/commits/{}",
            commit('c').as_str()
        )
    }

    async fn diffing(server: &MockServer, route: String, body: &str) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("accept", DIFF))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn diff(service: &PullRequestService, limit: usize) -> PullRequestResult<Excerpt> {
        service
            .fetch_commit_diff(&project(service), &token(), &commit('c'), limit)
            .await
    }

    #[tokio::test]
    async fn a_commit_s_diff_within_the_budget_is_whole() {
        let body = "diff --git a/x b/x\n+fn main() {}\n";
        let server = MockServer::start().await;
        diffing(&server, diff_path(""), body).await;
        let service = stand_in(&server).await;

        let read = diff(&service, body.len()).await.unwrap();

        assert_eq!(read.text, body);
        assert!(!read.truncated);
    }

    #[tokio::test]
    async fn a_commit_s_diff_is_cut_at_the_budget() {
        let body = "diff --git a/x b/x\n+".repeat(20);
        let server = MockServer::start().await;
        diffing(&server, diff_path(""), &body).await;
        let service = stand_in(&server).await;

        let read = diff(&service, 50).await.unwrap();

        assert!(read.truncated);
        assert_eq!(read.text, body[..50]);
        assert!(read.to_string().ends_with(Excerpt::MARKER), "{read}");
    }

    #[tokio::test]
    async fn a_commit_the_repository_does_not_hold_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(diff_path("")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = diff(&service, 50).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_commit_s_diff_is_read_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        diffing(&server, diff_path("/api/v3"), "diff --git a/x b/x\n").await;
        let service = enterprise(&server).await;

        let read = diff(&service, 1024).await.unwrap();

        assert_eq!(read.text, "diff --git a/x b/x\n");
    }
}
