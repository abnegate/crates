use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::Deployment;
use crate::pull_request::DeploymentStatus;
use crate::pull_request::Page;
use crate::pull_request::github_deployment::GitHubDeployment;
use crate::pull_request::github_deployment_status::GitHubDeploymentStatus;
use std::num::NonZeroU8;
use std::num::NonZeroU32;

/// Statuses read per deployment: GitHub lists the newest first, and only it
/// is kept.
const LATEST: &str = "1";

impl PullRequestService {
    /// Page `page` of the deployments GitHub lists for `commit` on
    /// `repository`, newest first, at `per_page` rows a page up to GitHub's
    /// hundred, each with the latest status its provider gave.
    ///
    /// Each deployment's status is a read of its own, made one after another
    /// so a page never bursts past GitHub's secondary rate limit. A row naming
    /// no identifier or commit is [`PullRequestError::GitHubApi`], one naming a
    /// commit this crate does not accept is [`PullRequestError::Parse`], and a
    /// status read that fails, such as one GitHub answers with
    /// [`PullRequestError::NotFound`], fails the page: never a page without
    /// the row or its status. Whether each row is of `commit` is the caller's
    /// to judge, from [`Deployment::sha`].
    pub async fn list_deployments(
        &self,
        repository: &Repository,
        token: &SecretValue,
        commit: &CommitSha,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Deployment>> {
        let asked = usize::from(per_page.get()).min(PAGE_SIZE);
        let mut url = self.deployments(repository, &[]);
        url.query_pairs_mut()
            .append_pair("sha", commit.as_str())
            .append_pair("per_page", &asked.to_string())
            .append_pair("page", &page.to_string());
        let rows: Vec<GitHubDeployment> = self.get(url, token).await?;
        let read = rows.len();

        let mut deployments = Vec::with_capacity(read);
        for row in rows {
            let id = row.id.to_string();
            let mut deployment =
                row.answered_at(self.deployments(repository, &[&id]).to_string())?;
            deployment.status = self.latest_status(repository, token, &id).await?;
            deployments.push(deployment);
        }
        Ok(Page::after(deployments, page, read, asked))
    }

    /// The newest status of the deployment GitHub identifies by `id`, or
    /// `None` before its provider gave any.
    async fn latest_status(
        &self,
        repository: &Repository,
        token: &SecretValue,
        id: &str,
    ) -> PullRequestResult<Option<DeploymentStatus>> {
        let mut url = self.deployments(repository, &[id, "statuses"]);
        url.query_pairs_mut().append_pair("per_page", LATEST);
        let statuses: Vec<GitHubDeploymentStatus> = self.get(url, token).await?;
        Ok(statuses.into_iter().next().map(DeploymentStatus::from))
    }

    /// The REST endpoint for `repository`'s deployments, followed by `rest`.
    fn deployments(&self, repository: &Repository, rest: &[&str]) -> Url {
        let segments: Vec<&str> = [
            "repos",
            repository.owner(),
            repository.name(),
            "deployments",
        ]
        .into_iter()
        .chain(rest.iter().copied())
        .collect();
        self.origin.endpoint(&segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
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
    use wiremock::matchers::query_param;

    const LISTED: &str = "/repos/acme/project/deployments";

    fn statuses(id: u64) -> String {
        format!("{LISTED}/{id}/statuses")
    }

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(per_page: u8) -> NonZeroU8 {
        NonZeroU8::new(per_page).unwrap()
    }

    fn listed(id: u64, sha: &CommitSha) -> Value {
        json!({ "id": id, "sha": sha.as_str(), "environment": "production" })
    }

    async fn listing(server: &MockServer, sha: &CommitSha, number: u32, body: Value) {
        Mock::given(method("GET"))
            .and(path(LISTED))
            .and(query_param("sha", sha.as_str()))
            .and(query_param("page", number.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn answering_statuses(server: &MockServer, id: u64, body: Value) {
        Mock::given(method("GET"))
            .and(path(statuses(id)))
            .and(query_param("per_page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn listed_page(
        server: &MockServer,
        sha: &CommitSha,
        number: u32,
        per_page: u8,
    ) -> PullRequestResult<Page<Deployment>> {
        let service = stand_in(server).await;
        service
            .list_deployments(
                &project(&service),
                &token(),
                sha,
                page(number),
                rows(per_page),
            )
            .await
    }

    #[tokio::test]
    async fn a_deployment_carries_the_newest_status_read_one_row_at_a_time() {
        let server = MockServer::start().await;
        let sha = commit('a');
        Mock::given(method("GET"))
            .and(path(LISTED))
            .and(query_param("sha", sha.as_str()))
            .and(query_param("per_page", "30"))
            .and(query_param("page", "1"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": 7,
                "sha": sha.as_str(),
                "ref": "main",
                "environment": "production",
                "created_at": "2026-09-05T00:00:00Z",
                "updated_at": "2026-09-05T00:05:00Z",
                "url": "https://api.github.com/repos/acme/project/deployments/7",
            }])))
            .expect(1)
            .mount(&server)
            .await;
        answering_statuses(
            &server,
            7,
            json!([
                {
                    "state": "success",
                    "description": "Deployed",
                    "environment_url": "https://project.example",
                    "log_url": "https://ci.example/7",
                    "created_at": "2026-09-05T00:05:00Z",
                },
                { "state": "pending", "created_at": "2026-09-05T00:00:00Z" },
            ]),
        )
        .await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        let expected = Deployment::new(7, sha, format!("{}{LISTED}/7", server.uri()))
            .with_reference("main")
            .with_environment("production")
            .with_created_at("2026-09-05T00:00:00Z")
            .with_updated_at("2026-09-05T00:05:00Z")
            .with_status(
                DeploymentStatus::new("success", "2026-09-05T00:05:00Z")
                    .with_description("Deployed")
                    .with_environment_url("https://project.example")
                    .with_log_url("https://ci.example/7"),
            );
        assert_eq!(listed, Page::new(vec![expected], None));
    }

    #[tokio::test]
    async fn a_deployment_with_no_status_yet_has_none() {
        let server = MockServer::start().await;
        let sha = commit('b');
        listing(&server, &sha, 1, json!([listed(3, &sha)])).await;
        answering_statuses(&server, 3, json!([])).await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].status, None);
    }

    #[tokio::test]
    async fn a_full_page_is_followed_by_the_next_and_a_short_one_is_the_last() {
        let server = MockServer::start().await;
        let sha = commit('c');
        listing(&server, &sha, 1, json!([listed(1, &sha), listed(2, &sha)])).await;
        listing(&server, &sha, 2, json!([listed(3, &sha)])).await;
        for id in 1..=3 {
            answering_statuses(&server, id, json!([])).await;
        }

        let first = listed_page(&server, &sha, 1, 2).await.unwrap();
        let second = listed_page(&server, &sha, 2, 2).await.unwrap();

        assert_eq!(first.next, Some(page(2)), "a full page is followed");
        assert_eq!(second.next, None, "a short page is the last");
        let identifiers: Vec<u64> = first
            .items
            .iter()
            .chain(&second.items)
            .map(|deployment| deployment.id)
            .collect();
        assert_eq!(identifiers, [1, 2, 3]);
    }

    #[tokio::test]
    async fn rows_asked_for_never_exceed_what_github_sends_a_page() {
        let server = MockServer::start().await;
        let sha = commit('d');
        Mock::given(method("GET"))
            .and(path(LISTED))
            .and(query_param("per_page", PAGE_SIZE.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        let listed = listed_page(&server, &sha, 1, u8::MAX).await.unwrap();

        assert_eq!(listed, Page::new(Vec::new(), None));
    }

    #[tokio::test]
    async fn a_status_github_does_not_find_fails_the_page_rather_than_shortening_it() {
        let server = MockServer::start().await;
        let sha = commit('e');
        listing(&server, &sha, 1, json!([listed(1, &sha), listed(2, &sha)])).await;
        answering_statuses(&server, 1, json!([{ "state": "success" }])).await;
        Mock::given(method("GET"))
            .and(path(statuses(2)))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .mount(&server)
            .await;

        let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::NotFound),
            "a missing status is not a page without it: {refused:?}"
        );
    }

    #[tokio::test]
    async fn the_least_github_may_say_of_a_deployment_still_reads() {
        let server = MockServer::start().await;
        let sha = commit('f');
        listing(&server, &sha, 1, json!([listed(7, &sha)])).await;
        answering_statuses(
            &server,
            7,
            json!([{ "state": "pending", "created_at": "2026-09-05T00:00:00Z" }]),
        )
        .await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        let expected = Deployment::new(7, sha, format!("{}{LISTED}/7", server.uri()))
            .with_environment("production")
            .with_status(DeploymentStatus::new("pending", "2026-09-05T00:00:00Z"));
        assert_eq!(listed, Page::new(vec![expected], None));
    }

    #[tokio::test]
    async fn a_deployment_of_a_commit_this_crate_does_not_accept_is_a_parse_error() {
        let server = MockServer::start().await;
        let sha = commit('0');
        listing(&server, &sha, 1, json!([{ "id": 7, "sha": "abc" }])).await;
        answering_statuses(&server, 7, json!([])).await;

        let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

        assert!(
            matches!(&refused, PullRequestError::Parse(ParseError::CommitSha(value)) if value == "abc"),
            "a malformed commit is a parse error: {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_deployment_naming_no_identifier_or_commit_is_unreadable() {
        for row in [json!({ "sha": "1".repeat(40) }), json!({ "id": 7 })] {
            let server = MockServer::start().await;
            let sha = commit('1');
            listing(&server, &sha, 1, json!([row])).await;
            answering_statuses(&server, 7, json!([])).await;

            let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

            assert!(
                matches!(&refused, PullRequestError::GitHubApi(message) if message == UNREADABLE),
                "{row} is unreadable: {refused:?}"
            );
        }
    }
}
