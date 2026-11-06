use crate::pull_request::Page;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::Installation;
use crate::pull_request::application::InstallationId;
use crate::pull_request::application::github_installation::GitHubInstallation;
use crate::pull_request::application::service::ApplicationService;
use crate::pull_request::application::service::acknowledged;
use crate::pull_request::application::service::answered;
use crate::pull_request::application::service::parsed;
use crate::pull_request::service::asked;
use crate::pull_request::service::paged;
use abnegate_secret::SecretValue;
use reqwest::Method;
use std::num::NonZeroU8;
use std::num::NonZeroU32;

impl ApplicationService {
    /// The App's installation `installation`, with `jwt` from
    /// [`crate::pull_request::ApplicationClaims::sign`].
    ///
    /// An installation GitHub does not know, or one of another App, is
    /// [`ApplicationError::Status`] with GitHub's 404, and any other refusal,
    /// a redirect included, is [`ApplicationError::Status`] with its own
    /// status. An installation that leaves out the App it installs or the
    /// account it is on, names an account GitHub would never put in a URL, or
    /// was suspended at no instant this crate can read, is
    /// [`ApplicationError::Response`].
    pub async fn fetch_installation(
        &self,
        jwt: &SecretValue,
        installation: InstallationId,
    ) -> Result<Installation, ApplicationError> {
        let installation = installation.get().to_string();
        let url = self.endpoint(&["app", "installations", &installation])?;
        let body = answered(self.authorized(Method::GET, url, jwt)).await?;
        parsed::<GitHubInstallation>(&body)?.try_into()
    }

    /// Page `page` of the App's installations, in the order GitHub lists
    /// them, at `per_page` rows a page up to GitHub's hundred, with `jwt`
    /// from [`crate::pull_request::ApplicationClaims::sign`].
    ///
    /// A row this crate cannot read, such as one that leaves out the App it
    /// installs or the account it is on, is [`ApplicationError::Response`],
    /// never a page without it. A refusal, a redirect included, is
    /// [`ApplicationError::Status`].
    pub async fn list_installations(
        &self,
        jwt: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> Result<Page<Installation>, ApplicationError> {
        let asked = asked(per_page);
        let url = paged(self.endpoint(&["app", "installations"])?, page, asked);
        let rows: Vec<GitHubInstallation> =
            parsed(&answered(self.authorized(Method::GET, url, jwt)).await?)?;
        let read = rows.len();

        let installations = rows
            .into_iter()
            .map(Installation::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Page::after(installations, page, read, asked))
    }

    /// Uninstall the App from `installation`, with `jwt` from
    /// [`crate::pull_request::ApplicationClaims::sign`].
    ///
    /// An installation already gone, which GitHub answers with a 404, is not
    /// an error, so a second delete after a lost answer succeeds. Any other
    /// refusal, a redirect included, is [`ApplicationError::Status`].
    pub async fn delete_installation(
        &self,
        jwt: &SecretValue,
        installation: InstallationId,
    ) -> Result<(), ApplicationError> {
        let installation = installation.get().to_string();
        let url = self.endpoint(&["app", "installations", &installation])?;
        acknowledged(self.authorized(Method::DELETE, url, jwt)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::API_VERSION;
    use crate::pull_request::application::API_VERSION_HEADER;
    use crate::pull_request::application::Account;
    use crate::pull_request::application::AccountType;
    use crate::pull_request::application::ApplicationId;
    use crate::pull_request::application::RepositorySelection;
    use crate::pull_request::application::fixtures::GUARD;
    use crate::pull_request::application::fixtures::at;
    use crate::pull_request::application::service::fixtures::installation;
    use crate::pull_request::application::service::fixtures::jwt;
    use crate::pull_request::application::service::fixtures::stalled;
    use crate::pull_request::application::service::fixtures::stand_in;
    use crate::pull_request::service::ACCEPT;
    use crate::pull_request::service::MAXIMUM_ANSWER_BYTES;
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

    /// Where installation 67890 is addressed.
    const INSTALLATION_PATH: &str = "/app/installations/67890";

    /// Where the App's installations are listed.
    const INSTALLATIONS_PATH: &str = "/app/installations";

    /// Where acme manages installation `id`.
    fn html_url(id: u64) -> String {
        format!("https://github.com/organizations/acme/settings/installations/{id}")
    }

    /// Installation `id` of App 12345 on acme, as GitHub's documentation
    /// shows one.
    fn row(id: u64) -> Value {
        json!({
            "id": id,
            "account": {
                "login": "acme",
                "id": 1,
                "node_id": "MDQ6VXNlcjE=",
                "avatar_url": "https://github.com/images/error/acme_happy.gif",
                "type": "Organization",
                "site_admin": false
            },
            "access_tokens_url": format!("https://api.github.com/app/installations/{id}/access_tokens"),
            "repositories_url": "https://api.github.com/installation/repositories",
            "html_url": html_url(id),
            "app_id": 12345,
            "target_id": 1,
            "target_type": "Organization",
            "permissions": {"checks": "write", "metadata": "read", "contents": "read"},
            "events": ["push", "pull_request"],
            "repository_selection": "selected",
            "created_at": "2018-02-09T20:51:14Z",
            "updated_at": "2018-02-09T20:51:14Z",
            "app_slug": "octocat-app",
            "suspended_at": null,
            "suspended_by": null
        })
    }

    /// `count` installations, numbered from 1.
    fn rows(count: u64) -> Value {
        Value::Array((1..=count).map(row).collect())
    }

    /// Installation `id` as [`row`] describes it.
    fn read(id: u64) -> Installation {
        Installation::new(
            InstallationId::new(id),
            ApplicationId::new(12345),
            Account::new("acme", AccountType::Organization),
            RepositorySelection::Selected,
            html_url(id),
        )
    }

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn per_page(rows: u8) -> NonZeroU8 {
        NonZeroU8::new(rows).unwrap()
    }

    /// A service asking a stand-in that answers every `GET` with `body`.
    async fn answering(server: &MockServer, body: Value) -> ApplicationService {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
        stand_in(server)
    }

    #[tokio::test]
    async fn an_installation_is_fetched_with_the_jwt() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(INSTALLATION_PATH))
            .and(header("Authorization", "Bearer header.payload.signature"))
            .and(header(API_VERSION_HEADER, API_VERSION))
            .and(header("Accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(row(67890)))
            .expect(1)
            .mount(&server)
            .await;

        let installation = stand_in(&server)
            .fetch_installation(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(installation, read(67890));
    }

    #[tokio::test]
    async fn a_suspended_installation_is_fetched_with_when() {
        let server = MockServer::start().await;
        let mut suspended = row(67890);
        suspended["suspended_at"] = json!("2026-01-15T12:00:00Z");
        suspended["suspended_by"] = json!({"login": "octocat", "type": "User"});

        let installation = answering(&server, suspended)
            .await
            .fetch_installation(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(installation.suspended_at, Some(at(1_768_478_400)));
    }

    #[tokio::test]
    async fn an_enterprise_installation_is_fetched_as_one() {
        let server = MockServer::start().await;
        let mut enterprise = row(67890);
        enterprise["target_type"] = json!("Enterprise");
        enterprise["account"] = json!({
            "id": 1,
            "slug": "acme-enterprise",
            "name": "Acme Enterprise",
            "node_id": "MDEwOkVudGVycHJpc2Ux",
            "html_url": "https://github.com/enterprises/acme-enterprise"
        });

        let installation = answering(&server, enterprise)
            .await
            .fetch_installation(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(
            installation.account,
            Account::new("acme-enterprise", AccountType::Enterprise)
        );
    }

    #[tokio::test]
    async fn an_installation_github_does_not_know_is_a_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(INSTALLATION_PATH))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .fetch_installation(&jwt(), installation())
            .await
            .expect_err("an unknown installation is a failure");

        assert!(matches!(error, ApplicationError::Status(404)), "{error:?}");
    }

    #[tokio::test]
    async fn a_refused_read_surfaces_as_a_status_without_the_url_or_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "Bad credentials: header.payload.signature",
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server);

        let errors = [
            service
                .fetch_installation(&jwt(), installation())
                .await
                .expect_err("a 401 is a failure"),
            service
                .list_installations(&jwt(), page(1), per_page(30))
                .await
                .expect_err("a 401 is a failure"),
        ];

        for error in errors {
            let rendered = format!("{error} {error:?}");
            assert!(matches!(error, ApplicationError::Status(401)), "{error:?}");
            assert!(!rendered.contains("header.payload.signature"), "{rendered}");
            assert!(!rendered.contains("/app/installations"), "{rendered}");
            assert!(!rendered.contains(&server.uri()), "{rendered}");
        }
    }

    #[tokio::test]
    async fn a_full_page_of_installations_is_followed_by_the_next() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(INSTALLATIONS_PATH))
            .and(query_param("per_page", "2"))
            .and(query_param("page", "2"))
            .and(header("Authorization", "Bearer header.payload.signature"))
            .and(header(API_VERSION_HEADER, API_VERSION))
            .and(header("Accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(rows(2)))
            .expect(1)
            .mount(&server)
            .await;

        let listed = stand_in(&server)
            .list_installations(&jwt(), page(2), per_page(2))
            .await
            .unwrap();

        assert_eq!(listed.items, vec![read(1), read(2)]);
        assert_eq!(listed.next, Some(page(3)));
    }

    #[tokio::test]
    async fn a_short_page_of_installations_is_the_last() {
        let server = MockServer::start().await;

        let listed = answering(&server, rows(1))
            .await
            .list_installations(&jwt(), page(2), per_page(2))
            .await
            .unwrap();

        assert_eq!(listed.items, vec![read(1)]);
        assert_eq!(listed.next, None);
    }

    #[tokio::test]
    async fn no_more_than_a_hundred_installations_are_asked_for_a_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(INSTALLATIONS_PATH))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rows(0)))
            .expect(1)
            .mount(&server)
            .await;

        let listed = stand_in(&server)
            .list_installations(&jwt(), page(1), per_page(255))
            .await
            .unwrap();

        assert!(listed.items.is_empty());
        assert_eq!(listed.next, None);
    }

    #[tokio::test]
    async fn one_unreadable_installation_fails_the_whole_page() {
        let mut no_application = row(2);
        no_application.as_object_mut().unwrap().remove("app_id");
        let mut no_account = row(2);
        no_account["account"] = Value::Null;

        for unreadable in [no_application, no_account] {
            let server = MockServer::start().await;
            let error = answering(&server, json!([row(1), unreadable, row(3)]))
                .await
                .list_installations(&jwt(), page(1), per_page(3))
                .await
                .expect_err("a page is never returned without a row it holds");

            assert!(matches!(error, ApplicationError::Response), "{error:?}");
        }
    }

    #[tokio::test]
    async fn an_answer_past_the_byte_limit_is_refused_whole() {
        let server = MockServer::start().await;
        let padding = " ".repeat(MAXIMUM_ANSWER_BYTES);
        for (answered, body) in [
            (INSTALLATION_PATH, row(67890)),
            (INSTALLATIONS_PATH, rows(1)),
        ] {
            Mock::given(method("GET"))
                .and(path(answered))
                .respond_with(
                    ResponseTemplate::new(200).set_body_string(format!("{body}{padding}")),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let service = stand_in(&server);

        let fetched = service.fetch_installation(&jwt(), installation()).await;
        let listed = service
            .list_installations(&jwt(), page(1), per_page(30))
            .await;

        assert!(
            matches!(fetched, Err(ApplicationError::Response)),
            "{fetched:?}"
        );
        assert!(
            matches!(listed, Err(ApplicationError::Response)),
            "{listed:?}"
        );
    }

    #[tokio::test]
    async fn a_redirected_read_is_refused_rather_than_followed() {
        let target = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(rows(1)))
            .expect(0)
            .mount(&target)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}{INSTALLATIONS_PATH}", target.uri())),
            )
            .expect(2)
            .mount(&server)
            .await;
        let service = stand_in(&server);

        let fetched = service.fetch_installation(&jwt(), installation()).await;
        let listed = service
            .list_installations(&jwt(), page(1), per_page(30))
            .await;

        assert!(
            matches!(fetched, Err(ApplicationError::Status(302))),
            "{fetched:?}"
        );
        assert!(
            matches!(listed, Err(ApplicationError::Status(302))),
            "{listed:?}"
        );
        target.verify().await;
    }

    #[tokio::test]
    async fn a_read_whose_body_stops_arriving_is_given_up_at_the_timeout() {
        let service = stalled(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n[",
        )
        .await;

        let fetched =
            tokio::time::timeout(GUARD, service.fetch_installation(&jwt(), installation()))
                .await
                .expect("the body read is bounded by the service's timeout");
        let listed = tokio::time::timeout(
            GUARD,
            service.list_installations(&jwt(), page(1), per_page(30)),
        )
        .await
        .expect("the body read is bounded by the service's timeout");

        assert!(
            matches!(fetched, Err(ApplicationError::Transport(_))),
            "{fetched:?}"
        );
        assert!(
            matches!(listed, Err(ApplicationError::Transport(_))),
            "{listed:?}"
        );
    }

    #[tokio::test]
    async fn a_deleted_installation_is_done() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(INSTALLATION_PATH))
            .and(header("Authorization", "Bearer header.payload.signature"))
            .and(header(API_VERSION_HEADER, API_VERSION))
            .and(header("Accept", ACCEPT))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        stand_in(&server)
            .delete_installation(&jwt(), installation())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_installation_already_gone_is_done() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(INSTALLATION_PATH))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
            })))
            .expect(1)
            .mount(&server)
            .await;

        stand_in(&server)
            .delete_installation(&jwt(), installation())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_refusal_surfaces_as_a_status_without_the_url_or_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "Bad credentials: header.payload.signature",
            })))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .delete_installation(&jwt(), installation())
            .await
            .expect_err("a 401 is a failure");

        let rendered = error.to_string();
        assert!(matches!(error, ApplicationError::Status(401)), "{error:?}");
        assert!(!rendered.contains("header.payload.signature"));
        assert!(!rendered.contains("/app/installations"));
        assert!(!rendered.contains(&server.uri()));
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        for status in [302, 307] {
            let target = MockServer::start().await;
            Mock::given(any())
                .respond_with(ResponseTemplate::new(204))
                .expect(0)
                .mount(&target)
                .await;
            let server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("Location", format!("{}{INSTALLATION_PATH}", target.uri())),
                )
                .expect(1)
                .mount(&server)
                .await;

            let error = stand_in(&server)
                .delete_installation(&jwt(), installation())
                .await
                .expect_err("a redirect must not carry the JWT onward");

            assert!(
                matches!(error, ApplicationError::Status(refused) if refused == status),
                "{status}: {error:?}"
            );
            target.verify().await;
        }
    }

    #[tokio::test]
    async fn an_unreachable_host_surfaces_without_the_credentialed_url() {
        let error = ApplicationService::standing_in_for("http://127.0.0.1:1")
            .unwrap()
            .delete_installation(&jwt(), installation())
            .await
            .expect_err("a closed port is a failure");

        let rendered = error.to_string();
        assert!(matches!(error, ApplicationError::Transport(_)), "{error:?}");
        assert!(
            !rendered.contains("127.0.0.1") && !rendered.contains("/app/installations"),
            "the request URL survived into the error: {rendered}"
        );
        assert!(!rendered.contains("header.payload.signature"));
    }

    #[tokio::test]
    async fn an_answer_that_never_arrives_is_given_up_at_the_timeout() {
        let service = stalled("").await;

        let deleted =
            tokio::time::timeout(GUARD, service.delete_installation(&jwt(), installation()))
                .await
                .expect("the wait for an answer is bounded by the service's timeout");

        assert!(
            matches!(deleted, Err(ApplicationError::Transport(_))),
            "{deleted:?}"
        );
    }
}
