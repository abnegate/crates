use crate::pull_request::Page;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::InstalledRepository;
use crate::pull_request::application::github_installation_repositories::GitHubInstallationRepositories;
use crate::pull_request::application::service::ApplicationService;
use crate::pull_request::application::service::answered;
use crate::pull_request::application::service::parsed;
use crate::pull_request::service::asked;
use crate::pull_request::service::paged;
use abnegate_secret::SecretValue;
use reqwest::Method;
use std::num::NonZeroU8;
use std::num::NonZeroU32;

impl ApplicationService {
    /// Page `page` of the repositories an installation can reach, at
    /// `per_page` rows a page up to GitHub's hundred, with `token`, that
    /// installation's token from
    /// [`crate::pull_request::InstallationIssuer::issue`]. The App's JWT is
    /// not a credential GitHub accepts here.
    ///
    /// Whether another page follows is judged by how many rows GitHub sent,
    /// never by the `total_count` it sends beside them. A row this crate
    /// cannot read, such as one that leaves out whether it is private, names
    /// an owner or repository GitHub does not allow, or defaults to a branch
    /// `git check-ref-format` would refuse, is [`ApplicationError::Response`],
    /// never a page without it. A default branch only a command line would
    /// misread, such as `+release`, is listed as GitHub spells it.
    pub async fn list_installation_repositories(
        &self,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> Result<Page<InstalledRepository>, ApplicationError> {
        let asked = asked(per_page);
        let url = paged(
            self.endpoint(&["installation", "repositories"])?,
            page,
            asked,
        );
        let answer: GitHubInstallationRepositories =
            parsed(&answered(self.authorized(Method::GET, url, token)).await?)?;
        let read = answer.repositories.len();

        let repositories = answer
            .repositories
            .into_iter()
            .map(InstalledRepository::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Page::after(repositories, page, read, asked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Repository;
    use crate::pull_request::application::API_VERSION;
    use crate::pull_request::application::API_VERSION_HEADER;
    use crate::pull_request::application::fixtures::GUARD;
    use crate::pull_request::application::service::fixtures::stalled;
    use crate::pull_request::application::service::fixtures::stand_in;
    use crate::pull_request::application::service::fixtures::token;
    use crate::pull_request::service::ACCEPT;
    use crate::pull_request::service::MAXIMUM_ANSWER_BYTES;
    use crate::remote_branch_name::RemoteBranchName;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    /// Where an installation's repositories are listed.
    const REPOSITORIES_PATH: &str = "/installation/repositories";

    /// The start of a page of repositories that promises more than it sends.
    const STALLED: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"repositories\":[";

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    fn installed(id: u64) -> InstalledRepository {
        defaulting_to(id, "main")
    }

    fn defaulting_to(id: u64, default_branch: &str) -> InstalledRepository {
        let name = format!("project-{id}");
        InstalledRepository::new(
            id,
            Repository::new("acme", &name).unwrap(),
            RemoteBranchName::parse(default_branch).unwrap(),
            format!("https://github.com/acme/{name}"),
        )
        .with_private(id.is_multiple_of(2))
    }

    fn described(repository: &InstalledRepository) -> Value {
        json!({
            "id": repository.id(),
            "node_id": "MDEwOlJlcG9zaXRvcnkxMjk2MjY5",
            "name": repository.name(),
            "full_name": repository.full_name(),
            "owner": {
                "login": repository.owner(),
                "id": 1,
                "type": "Organization",
                "site_admin": false
            },
            "private": repository.private(),
            "archived": repository.archived(),
            "html_url": repository.html_url(),
            "default_branch": repository.default_branch().as_str(),
            "visibility": if repository.private() { "private" } else { "public" },
            "permissions": { "admin": false, "push": true, "pull": true }
        })
    }

    fn answer(total_count: usize, repositories: Vec<Value>) -> Value {
        json!({
            "total_count": total_count,
            "repository_selection": "selected",
            "repositories": repositories,
        })
    }

    async fn listing(server: &MockServer, asked: &str, answer: Value) {
        Mock::given(method("GET"))
            .and(path(REPOSITORIES_PATH))
            .and(query_param("per_page", asked))
            .and(query_param("page", "2"))
            .and(header("Authorization", "Bearer ghs_installationtokenvalue"))
            .and(header("Accept", ACCEPT))
            .and(header(API_VERSION_HEADER, API_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn list(
        service: &ApplicationService,
        per_page: u8,
    ) -> Result<Page<InstalledRepository>, ApplicationError> {
        service
            .list_installation_repositories(&token(), page(2), rows(per_page))
            .await
    }

    #[tokio::test]
    async fn a_full_page_is_asked_for_with_the_installation_token_and_followed() {
        let server = MockServer::start().await;
        let repositories: Vec<InstalledRepository> = (1..=2).map(installed).collect();
        listing(
            &server,
            "2",
            answer(7, repositories.iter().map(described).collect()),
        )
        .await;

        let listed = list(&stand_in(&server), 2).await.unwrap();

        assert_eq!(listed, Page::new(repositories, Some(page(3))));
    }

    #[tokio::test]
    async fn a_short_page_is_the_last() {
        let server = MockServer::start().await;
        let repositories = vec![installed(1)];
        listing(
            &server,
            "2",
            answer(3, repositories.iter().map(described).collect()),
        )
        .await;

        let listed = list(&stand_in(&server), 2).await.unwrap();

        assert_eq!(listed, Page::new(repositories, None));
    }

    #[tokio::test]
    async fn no_more_rows_are_asked_for_than_github_sends() {
        let server = MockServer::start().await;
        listing(&server, "100", answer(0, Vec::new())).await;

        let listed = list(&stand_in(&server), u8::MAX).await.unwrap();

        assert_eq!(listed, Page::new(Vec::new(), None));
    }

    #[tokio::test]
    async fn paging_follows_the_rows_sent_rather_than_the_total_count() {
        for (total_count, sent, next) in [(1, 2, Some(page(3))), (50, 1, None)] {
            let server = MockServer::start().await;
            let repositories: Vec<InstalledRepository> = (1..=sent).map(installed).collect();
            listing(
                &server,
                "2",
                answer(total_count, repositories.iter().map(described).collect()),
            )
            .await;

            let listed = list(&stand_in(&server), 2).await.unwrap();

            assert_eq!(
                listed.next, next,
                "{sent} rows sent beside a total_count of {total_count}"
            );
        }
    }

    #[tokio::test]
    async fn a_default_branch_a_command_line_would_misread_does_not_fail_the_page() {
        let server = MockServer::start().await;
        let repositories = vec![defaulting_to(1, "+release"), defaulting_to(2, "-dash")];
        listing(
            &server,
            "2",
            answer(2, repositories.iter().map(described).collect()),
        )
        .await;

        let listed = list(&stand_in(&server), 2).await.unwrap();

        assert_eq!(listed, Page::new(repositories, Some(page(3))));
        assert_eq!(listed.items[0].default_branch().as_str(), "+release");
        assert_eq!(listed.items[1].default_branch().as_str(), "-dash");
    }

    #[tokio::test]
    async fn a_row_this_crate_cannot_read_fails_the_whole_page() {
        let mut refused_owner = described(&installed(2));
        refused_owner["owner"] = json!({ "login": "acme/other" });
        let mut without_private = described(&installed(2));
        without_private.as_object_mut().unwrap().remove("private");
        let mut refused_branch = described(&installed(2));
        refused_branch["default_branch"] = json!("a..b");
        for (case, row) in [
            ("a refused owner", refused_owner),
            ("a missing private flag", without_private),
            ("a default branch git refuses", refused_branch),
        ] {
            let server = MockServer::start().await;
            listing(&server, "2", answer(2, vec![described(&installed(1)), row])).await;

            let error = list(&stand_in(&server), 2).await.unwrap_err();

            assert!(
                matches!(error, ApplicationError::Response),
                "{case}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_answer_without_its_repositories_is_unreadable() {
        let server = MockServer::start().await;
        listing(
            &server,
            "2",
            json!({ "total_count": 0, "repository_selection": "all" }),
        )
        .await;

        let error = list(&stand_in(&server), 2).await.unwrap_err();

        assert!(matches!(error, ApplicationError::Response), "{error:?}");
    }

    #[tokio::test]
    async fn a_refusal_surfaces_as_a_status_without_the_token_or_the_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "Bad credentials: ghs_installationtokenvalue",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = list(&stand_in(&server), 2)
            .await
            .expect_err("a 401 is a failure");

        let rendered = format!("{error} {error:?}");
        assert!(matches!(error, ApplicationError::Status(401)), "{error:?}");
        assert!(!rendered.contains("ghs_installationtokenvalue"));
        assert!(!rendered.contains(REPOSITORIES_PATH));
        assert!(!rendered.contains(&server.uri()));
    }

    #[tokio::test]
    async fn an_answer_past_the_byte_limit_is_refused_whole() {
        let server = MockServer::start().await;
        let padding = " ".repeat(MAXIMUM_ANSWER_BYTES);
        let body = format!(
            r#"{{"total_count":1,"repositories":[{}]}}{padding}"#,
            described(&installed(1))
        );
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let error = list(&stand_in(&server), 2)
            .await
            .expect_err("an answer past the limit is never parsed in part");

        let rendered = format!("{error} {error:?}");
        assert!(matches!(error, ApplicationError::Response), "{error:?}");
        assert!(!rendered.contains("ghs_installationtokenvalue"));
        assert!(!rendered.contains("project-1"));
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        for status in [302, 307] {
            let target = MockServer::start().await;
            Mock::given(any())
                .respond_with(ResponseTemplate::new(200).set_body_json(answer(0, Vec::new())))
                .expect(0)
                .mount(&target)
                .await;
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("Location", format!("{}{REPOSITORIES_PATH}", target.uri())),
                )
                .expect(1)
                .mount(&server)
                .await;

            let error = list(&stand_in(&server), 2)
                .await
                .expect_err("a redirect must not carry the token onward");

            let rendered = format!("{error} {error:?}");
            assert!(
                matches!(error, ApplicationError::Status(refused) if refused == status),
                "{status}: {error:?}"
            );
            assert!(!rendered.contains("ghs_installationtokenvalue"));
            assert!(!rendered.contains(&target.uri()));
            target.verify().await;
        }
    }

    #[tokio::test]
    async fn an_unreachable_host_surfaces_without_the_credentialed_url() {
        let error = list(
            &ApplicationService::standing_in_for("http://127.0.0.1:1").unwrap(),
            2,
        )
        .await
        .expect_err("a closed port is a failure");

        let rendered = format!("{error} {error:?}");
        assert!(matches!(error, ApplicationError::Transport(_)), "{error:?}");
        assert!(
            !rendered.contains("127.0.0.1") && !rendered.contains(REPOSITORIES_PATH),
            "the request URL survived into the error: {rendered}"
        );
        assert!(!rendered.contains("ghs_installationtokenvalue"));
    }

    #[tokio::test]
    async fn an_answer_that_stops_arriving_is_given_up_at_the_timeout() {
        let service = stalled(STALLED).await;

        let listed = tokio::time::timeout(GUARD, list(&service, 2))
            .await
            .expect("the body read is bounded by the service's timeout");

        assert!(
            matches!(listed, Err(ApplicationError::Transport(_))),
            "{listed:?}"
        );
    }
}
