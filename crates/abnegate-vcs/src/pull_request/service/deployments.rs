use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::Deployment;
use crate::pull_request::DeploymentStatus;
use crate::pull_request::Page;
use crate::pull_request::github_deployment::GitHubDeployment;
use crate::pull_request::github_deployment_status::GitHubDeploymentStatus;

/// Statuses read per deployment: GitHub lists the newest first, and only it
/// is kept.
const LATEST: &str = "1";

impl PullRequestService {
    /// Page `page` of the deployments GitHub lists for `commit` on
    /// `repository`, newest first, at `per_page` rows a page up to GitHub's
    /// hundred, each with the latest status its provider gave.
    ///
    /// Each deployment's status is a read of its own, made one after another
    /// so a page never bursts past GitHub's secondary rate limit. A row or a
    /// status that leaves out anything GitHub sends on every one, such as a
    /// deployment's identifier, commit or environment or a status's state, is
    /// [`PullRequestError::GitHubApi`], one naming a commit this crate does
    /// not accept is [`PullRequestError::Parse`], and a status read that
    /// fails, such as one GitHub answers with [`PullRequestError::NotFound`],
    /// fails the page: never a page without the row or its status. Whether
    /// each row is of `commit` is the caller's to judge, from
    /// [`Deployment::sha`].
    pub async fn list_deployments(
        &self,
        repository: &Repository,
        token: &SecretValue,
        commit: &CommitSha,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Deployment>> {
        let asked = asked(per_page);
        let mut url = self.repository_endpoint(repository, &["deployments"])?;
        url.query_pairs_mut().append_pair("sha", commit.as_str());
        let rows: Vec<GitHubDeployment> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let mut deployments = Vec::with_capacity(read);
        for row in rows {
            let id = row.id.to_string();
            let api_url = self.repository_endpoint(repository, &["deployments", &id])?;
            let mut deployment = row.answered_at(api_url.to_string())?;
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
        let mut url = self.repository_endpoint(repository, &["deployments", id, "statuses"])?;
        url.query_pairs_mut().append_pair("per_page", LATEST);
        let statuses: Vec<GitHubDeploymentStatus> = self.get(url, token).await?;
        Ok(statuses.into_iter().next().map(DeploymentStatus::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
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

    const LISTED: &str = "/repos/acme/project/deployments";

    const CREATED: &str = "2026-09-05T00:00:00Z";

    const UPDATED: &str = "2026-09-05T00:05:00Z";

    /// What GitHub sends on every deployment, without which a row is
    /// unreadable.
    const REQUIRED: &[&str] = &[
        "id",
        "sha",
        "ref",
        "environment",
        "created_at",
        "updated_at",
    ];

    /// What GitHub sends on every status, without which a status is
    /// unreadable.
    const REQUIRED_OF_A_STATUS: &[&str] = &["state", "description", "created_at"];

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
        json!({
            "id": id,
            "sha": sha.as_str(),
            "ref": "main",
            "environment": "production",
            "created_at": CREATED,
            "updated_at": UPDATED,
            "url": format!("https://api.github.com{LISTED}/{id}"),
        })
    }

    fn pending() -> Value {
        json!({ "state": "pending", "description": "", "created_at": CREATED })
    }

    fn deployed(server: &MockServer, id: u64, sha: CommitSha) -> Deployment {
        Deployment::new(
            id,
            sha,
            "main",
            "production",
            CREATED,
            UPDATED,
            format!("{}{LISTED}/{id}", server.uri()),
        )
    }

    async fn listing(server: &MockServer, sha: &CommitSha, number: u32, per_page: u8, body: Value) {
        Mock::given(method("GET"))
            .and(path(LISTED))
            .and(query_is(&[
                ("sha", sha.as_str()),
                ("per_page", &per_page.to_string()),
                ("page", &number.to_string()),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn answering_statuses(server: &MockServer, id: u64, body: Value, reads: u64) {
        Mock::given(method("GET"))
            .and(path(statuses(id)))
            .and(query_is(&[("per_page", LATEST)]))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(reads)
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

    fn unreadable(failure: &PullRequestError) -> bool {
        matches!(failure, PullRequestError::GitHubApi(message) if message == UNREADABLE)
    }

    #[tokio::test]
    async fn a_deployment_carries_the_newest_status_read_one_row_at_a_time() {
        let server = MockServer::start().await;
        let sha = commit('a');
        Mock::given(method("GET"))
            .and(path(LISTED))
            .and(query_is(&[
                ("sha", sha.as_str()),
                ("per_page", "30"),
                ("page", "1"),
            ]))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed(7, &sha)])))
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
                    "created_at": UPDATED,
                },
                pending(),
            ]),
            1,
        )
        .await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        let expected = deployed(&server, 7, sha).with_status(
            DeploymentStatus::new("success", "Deployed", UPDATED)
                .with_environment_url("https://project.example")
                .with_log_url("https://ci.example/7"),
        );
        assert_eq!(listed, Page::new(vec![expected], None));
    }

    #[tokio::test]
    async fn a_deployment_with_no_status_yet_has_none() {
        let server = MockServer::start().await;
        let sha = commit('b');
        listing(&server, &sha, 1, 30, json!([listed(3, &sha)])).await;
        answering_statuses(&server, 3, json!([]), 1).await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        assert_eq!(listed, Page::new(vec![deployed(&server, 3, sha)], None));
    }

    #[tokio::test]
    async fn a_full_page_is_followed_by_the_next_and_a_short_one_is_the_last() {
        let server = MockServer::start().await;
        let sha = commit('c');
        listing(
            &server,
            &sha,
            1,
            2,
            json!([listed(1, &sha), listed(2, &sha)]),
        )
        .await;
        listing(&server, &sha, 2, 2, json!([listed(3, &sha)])).await;
        for id in 1..=3 {
            answering_statuses(&server, id, json!([]), 1).await;
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
        listing(&server, &sha, 1, 100, json!([])).await;

        let listed = listed_page(&server, &sha, 1, u8::MAX).await.unwrap();

        assert_eq!(listed, Page::new(Vec::new(), None));
    }

    #[tokio::test]
    async fn a_status_github_does_not_find_fails_the_page_rather_than_shortening_it() {
        let server = MockServer::start().await;
        let sha = commit('e');
        listing(
            &server,
            &sha,
            1,
            30,
            json!([listed(1, &sha), listed(2, &sha)]),
        )
        .await;
        answering_statuses(&server, 1, json!([pending()]), 1).await;
        Mock::given(method("GET"))
            .and(path(statuses(2)))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::NotFound),
            "a missing status is not a page without it: {refused:?}"
        );
    }

    #[tokio::test]
    async fn what_a_provider_may_leave_out_of_a_status_is_read_as_not_said() {
        let server = MockServer::start().await;
        let sha = commit('f');
        let mut nulled = pending();
        nulled["environment_url"] = Value::Null;
        nulled["log_url"] = Value::Null;
        listing(
            &server,
            &sha,
            1,
            30,
            json!([listed(7, &sha), listed(8, &sha)]),
        )
        .await;
        answering_statuses(&server, 7, json!([pending()]), 1).await;
        answering_statuses(&server, 8, json!([nulled]), 1).await;

        let listed = listed_page(&server, &sha, 1, 30).await.unwrap();

        let status = DeploymentStatus::new("pending", "", CREATED);
        assert_eq!(
            listed,
            Page::new(
                vec![
                    deployed(&server, 7, sha.clone()).with_status(status.clone()),
                    deployed(&server, 8, sha).with_status(status),
                ],
                None
            )
        );
    }

    #[tokio::test]
    async fn a_deployment_of_a_commit_this_crate_does_not_accept_is_a_parse_error() {
        let server = MockServer::start().await;
        let sha = commit('0');
        let mut malformed = listed(7, &sha);
        malformed["sha"] = json!("abc");
        listing(&server, &sha, 1, 30, json!([malformed])).await;
        answering_statuses(&server, 7, json!([]), 0).await;

        let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

        assert!(
            matches!(&refused, PullRequestError::Parse(ParseError::CommitSha(value)) if value == "abc"),
            "a malformed commit is a parse error: {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_deployment_missing_what_github_always_sends_is_unreadable() {
        let sha = commit('1');
        for required in REQUIRED {
            let server = MockServer::start().await;
            let mut row = listed(7, &sha);
            row.as_object_mut().unwrap().remove(*required);
            listing(&server, &sha, 1, 30, json!([listed(6, &sha), row])).await;
            answering_statuses(&server, 6, json!([]), 0).await;
            answering_statuses(&server, 7, json!([]), 0).await;

            let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

            assert!(unreadable(&refused), "{required} missing: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_status_missing_what_github_always_sends_fails_the_page() {
        let sha = commit('2');
        for required in REQUIRED_OF_A_STATUS {
            let server = MockServer::start().await;
            let mut status = pending();
            status.as_object_mut().unwrap().remove(*required);
            listing(&server, &sha, 1, 30, json!([listed(7, &sha)])).await;
            answering_statuses(&server, 7, json!([status]), 1).await;

            let refused = listed_page(&server, &sha, 1, 30).await.unwrap_err();

            assert!(unreadable(&refused), "{required} missing: {refused:?}");
        }
    }
}
