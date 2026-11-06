use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::InstallationId;
use crate::pull_request::application::service::ApplicationService;
use crate::pull_request::application::service::acknowledged;
use abnegate_secret::SecretValue;
use reqwest::Method;

impl ApplicationService {
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
    use crate::pull_request::application::fixtures::GUARD;
    use crate::pull_request::application::service::fixtures::installation;
    use crate::pull_request::application::service::fixtures::jwt;
    use crate::pull_request::application::service::fixtures::stalled;
    use crate::pull_request::application::service::fixtures::stand_in;
    use crate::pull_request::service::ACCEPT;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// Where installation 67890 is deleted.
    const INSTALLATION_PATH: &str = "/app/installations/67890";

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
