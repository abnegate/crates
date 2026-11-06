use super::*;
use crate::pull_request::WebhookEvent;
use crate::pull_request::github_identified::GitHubIdentified;
use crate::pull_request::webhook_configuration::WebhookConfiguration;
use crate::pull_request::webhook_request::WebhookRequest;

/// What GitHub calls a webhook that delivers over HTTP.
const WEB: &str = "web";

impl PullRequestService {
    /// Register a webhook on `repository` that delivers `events` to `url` as
    /// JSON signed with `secret`, and return the identifier GitHub gave it.
    ///
    /// The token needs admin access to the repository, or the
    /// `admin:repo_hook` scope; without it GitHub answers
    /// [`PullRequestError::NotFound`] or [`PullRequestError::Forbidden`]. A
    /// webhook already delivering to `url` is refused by GitHub as
    /// [`PullRequestError::GitHubApi`] rather than registered twice.
    pub async fn create_webhook(
        &self,
        repository: &Repository,
        token: &SecretValue,
        url: &Url,
        secret: &SecretValue,
        events: &[WebhookEvent],
    ) -> PullRequestResult<u64> {
        let request = WebhookRequest {
            name: WEB,
            active: true,
            events,
            config: WebhookConfiguration::new(url, secret),
        };
        let created: GitHubIdentified = self
            .exchange(
                self.request(Method::POST, self.hooks(repository, &[])?, token, ACCEPT)
                    .json(&request),
            )
            .await?;
        Ok(created.id)
    }

    /// Point webhook `id` on `repository` at `url` and sign its deliveries
    /// with `secret` from now on.
    pub async fn update_webhook(
        &self,
        repository: &Repository,
        token: &SecretValue,
        id: u64,
        url: &Url,
        secret: &SecretValue,
    ) -> PullRequestResult<()> {
        let id = id.to_string();
        self.answered(
            self.request(
                Method::PATCH,
                self.hooks(repository, &[&id, "config"])?,
                token,
                ACCEPT,
            )
            .json(&WebhookConfiguration::new(url, secret)),
        )
        .await?;
        Ok(())
    }

    /// Remove webhook `id` from `repository`; one already gone is not an
    /// error.
    pub async fn delete_webhook(
        &self,
        repository: &Repository,
        token: &SecretValue,
        id: u64,
    ) -> PullRequestResult<()> {
        let id = id.to_string();
        let response = self
            .sent(self.request(
                Method::DELETE,
                self.hooks(repository, &[&id])?,
                token,
                ACCEPT,
            ))
            .await?;

        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(refusal(response).await)
    }

    fn hooks(&self, repository: &Repository, rest: &[&str]) -> PullRequestResult<Url> {
        let segments: Vec<&str> = ["hooks"].into_iter().chain(rest.iter().copied()).collect();
        self.repository_endpoint(repository, &segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::json;
    use std::mem::discriminant;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    const HOOKS: &str = "/repos/acme/project/hooks";
    const SIGNING: &str = "signing-secret-sentinel";

    fn delivery() -> Url {
        Url::parse("https://hooks.example.test/api/webhooks/sync/1/github").unwrap()
    }

    fn secret() -> SecretValue {
        SecretValue::new(SIGNING)
    }

    #[tokio::test]
    async fn a_webhook_is_registered_for_its_events_as_signed_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(HOOKS))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({
                "name": "web",
                "active": true,
                "events": ["issues", "issue_comment"],
                "config": {
                    "url": "https://hooks.example.test/api/webhooks/sync/1/github",
                    "content_type": "json",
                    "secret": SIGNING,
                    "insecure_ssl": "0",
                },
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": 4242 })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let id = service
            .create_webhook(
                &project(&service),
                &token(),
                &delivery(),
                &secret(),
                &[WebhookEvent::Issues, WebhookEvent::IssueComment],
            )
            .await
            .unwrap();

        assert_eq!(id, 4242);
    }

    #[tokio::test]
    async fn a_refused_registration_is_reported_as_what_it_is_without_the_secret() {
        for (response, expected) in [
            (ResponseTemplate::new(404), PullRequestError::NotFound),
            (ResponseTemplate::new(403), PullRequestError::Forbidden),
            (
                ResponseTemplate::new(401),
                PullRequestError::AuthenticationFailed,
            ),
            (
                ResponseTemplate::new(422).set_body_json(json!({
                    "message": "Validation Failed",
                    "errors": [{ "message": "Hook already exists on this repository" }],
                })),
                PullRequestError::GitHubApi(String::new()),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(HOOKS))
                .respond_with(response)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .create_webhook(
                    &project(&service),
                    &token(),
                    &delivery(),
                    &secret(),
                    &[WebhookEvent::Issues],
                )
                .await
                .unwrap_err();

            assert_eq!(
                discriminant(&failure),
                discriminant(&expected),
                "{expected:?}: {failure:?}"
            );
            assert!(!format!("{failure:?}").contains(SIGNING), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_registration_answered_without_an_id_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(HOOKS))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "web" })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .create_webhook(
                &project(&service),
                &token(),
                &delivery(),
                &secret(),
                &[WebhookEvent::Issues],
            )
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(_)),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_webhook_is_re_signed_through_its_configuration() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("{HOOKS}/4242/config")))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "url": "https://hooks.example.test/api/webhooks/sync/1/github",
                "content_type": "json",
                "secret": SIGNING,
                "insecure_ssl": "0",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .update_webhook(&project(&service), &token(), 4242, &delivery(), &secret())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn re_signing_a_webhook_that_is_gone_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("{HOOKS}/4242/config")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .update_webhook(&project(&service), &token(), 4242, &delivery(), &secret())
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_webhook_is_removed_and_one_already_gone_is_not_an_error() {
        for status in [204, 404] {
            let server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .and(path(format!("{HOOKS}/4242")))
                .and(header("authorization", "Bearer token"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            service
                .delete_webhook(&project(&service), &token(), 4242)
                .await
                .unwrap_or_else(|failure| panic!("{status}: {failure:?}"));
        }
    }

    #[tokio::test]
    async fn a_refused_removal_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("{HOOKS}/4242")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .delete_webhook(&project(&service), &token(), 4242)
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Forbidden),
            "{failure:?}"
        );
    }
}
