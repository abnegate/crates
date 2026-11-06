use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::RegisteredApplication;
use crate::pull_request::application::github_registered_application::GitHubRegisteredApplication;
use crate::pull_request::application::service::ApplicationService;
use crate::pull_request::application::service::answered;
use crate::pull_request::application::service::parsed;
use reqwest::Method;

impl ApplicationService {
    /// Register the GitHub App a manifest describes, from the `code` GitHub
    /// redirects back with once someone creates the App from that manifest.
    ///
    /// The request carries no credential: the code is itself the proof, and
    /// GitHub accepts it once, within an hour of handing it out. A code that
    /// is empty, `.` or `..` is [`ApplicationError::InvalidCode`] and is
    /// never sent. A code GitHub refuses, already used, expired, or never
    /// issued, is [`ApplicationError::Status`], as is any other refusal, a
    /// redirect included; no error repeats the code.
    ///
    /// The answer holds the App's client secret, webhook secret and private
    /// key, which GitHub never shows again, each held as a
    /// [`abnegate_secret::SecretValue`].
    pub async fn convert_manifest(
        &self,
        code: &str,
    ) -> Result<RegisteredApplication, ApplicationError> {
        let url = self
            .origin
            .endpoint(&["app-manifests", code, "conversions"])
            .map_err(|_| ApplicationError::InvalidCode)?;
        let body = answered(self.request(Method::POST, url)).await?;
        parsed::<GitHubRegisteredApplication>(&body)?.try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::ApplicationClaims;
    use crate::pull_request::application::API_VERSION;
    use crate::pull_request::application::API_VERSION_HEADER;
    use crate::pull_request::application::Account;
    use crate::pull_request::application::AccountType;
    use crate::pull_request::application::ApplicationId;
    use crate::pull_request::application::fixtures::GUARD;
    use crate::pull_request::application::fixtures::PRIVATE_KEY;
    use crate::pull_request::application::fixtures::at;
    use crate::pull_request::application::service::fixtures::stalled;
    use crate::pull_request::application::service::fixtures::stand_in;
    use crate::pull_request::service::ACCEPT;
    use crate::pull_request::service::MAXIMUM_ANSWER_BYTES;
    use abnegate_secret::OptionalSecretExtension;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// The code GitHub hands back after the App is created.
    const CODE: &str = "a180b1a3d263c81bc6441d7b990bae27d4c10679";

    /// Where [`CODE`] is converted.
    const CONVERSION_PATH: &str =
        "/app-manifests/a180b1a3d263c81bc6441d7b990bae27d4c10679/conversions";

    /// The client secret the conversion answers with, to see where it
    /// travels.
    const CLIENT_SECRET: &str = "1726be1638095a19edd134c77bde3aa2ece1e5d8";

    /// The webhook secret the conversion answers with, to see where it
    /// travels.
    const WEBHOOK_SECRET: &str = "e340154128314309424b7c8e90325147d99fdafa";

    /// The conversion's answer, shaped as GitHub documents it.
    fn conversion() -> Value {
        json!({
            "id": 12345,
            "slug": "octoapp",
            "node_id": "MDxOkludGVncmF0aW9uMQ==",
            "owner": {
                "login": "octocat-org",
                "id": 1,
                "node_id": "MDEyOk9yZ2FuaXphdGlvbjE=",
                "url": "https://api.github.com/orgs/octocat-org",
                "html_url": "https://github.com/octocat-org",
                "type": "Organization",
                "site_admin": false
            },
            "name": "Octocat App",
            "description": "",
            "external_url": "https://example.com",
            "html_url": "https://github.com/apps/octoapp",
            "created_at": "2017-07-08T16:18:44-04:00",
            "updated_at": "2017-07-08T16:18:44-04:00",
            "permissions": {
                "metadata": "read",
                "contents": "read",
                "issues": "write",
                "single_file": "write"
            },
            "events": ["push", "pull_request"],
            "client_id": "Iv1.8a61f9b3a7aba766",
            "client_secret": CLIENT_SECRET,
            "webhook_secret": WEBHOOK_SECRET,
            "pem": PRIVATE_KEY
        })
    }

    /// A server that converts any code into [`conversion`].
    async fn converting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(conversion()))
            .mount(&server)
            .await;
        server
    }

    /// Asserts `rendered` repeats no credential, the code, or where it was
    /// sent.
    fn assert_echoes_nothing(rendered: &str, server: &MockServer) {
        for leaked in [
            CODE,
            CLIENT_SECRET,
            WEBHOOK_SECRET,
            "BEGIN",
            "app-manifests",
            &server.uri(),
        ] {
            assert!(!rendered.contains(leaked), "{leaked} echoed: {rendered}");
        }
    }

    #[tokio::test]
    async fn a_converted_manifest_gives_every_field_of_the_registered_application() {
        let server = converting().await;

        let application = stand_in(&server).convert_manifest(CODE).await.unwrap();

        assert_eq!(application.id, ApplicationId::new(12345));
        assert_eq!(application.slug, "octoapp");
        assert_eq!(application.name, "Octocat App");
        assert_eq!(
            application.owner,
            Account::new("octocat-org", AccountType::Organization)
        );
        assert_eq!(application.html_url, "https://github.com/apps/octoapp");
        assert_eq!(application.client_id, "Iv1.8a61f9b3a7aba766");
        assert_eq!(application.client_secret.expose(), CLIENT_SECRET);
        assert_eq!(
            application.webhook_secret.expose_as_deref(),
            Some(WEBHOOK_SECRET)
        );
        assert_eq!(application.private_key.expose(), PRIVATE_KEY);
    }

    #[tokio::test]
    async fn the_converted_private_key_signs_the_apps_jwt() {
        let server = converting().await;

        let application = stand_in(&server).convert_manifest(CODE).await.unwrap();

        let jwt = ApplicationClaims::issue(application.id, at(1_700_000_000))
            .sign(&application.private_key)
            .expect("the converted key signs a JWT");
        assert_eq!(jwt.expose().split('.').count(), 3);
    }

    #[tokio::test]
    async fn the_conversion_carries_no_credential_but_asks_for_the_rest_api() {
        let server = converting().await;

        stand_in(&server).convert_manifest(CODE).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.path(), CONVERSION_PATH);
        assert!(request.headers.get("Authorization").is_none());
        assert_eq!(request.headers.get("Accept").unwrap(), ACCEPT);
        assert_eq!(
            request.headers.get(API_VERSION_HEADER).unwrap(),
            API_VERSION
        );
        assert!(request.body.is_empty());
    }

    #[tokio::test]
    async fn a_code_holding_a_slash_stays_one_path_segment() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app-manifests/a%2Fb/conversions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(conversion()))
            .expect(1)
            .mount(&server)
            .await;

        stand_in(&server).convert_manifest("a/b").await.unwrap();
    }

    #[tokio::test]
    async fn an_empty_or_dot_segment_code_is_refused_before_it_is_sent() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(201).set_body_json(conversion()))
            .expect(0)
            .mount(&server)
            .await;

        for code in ["", ".", ".."] {
            let error = stand_in(&server)
                .convert_manifest(code)
                .await
                .expect_err("a code that names another endpoint is never sent");

            assert!(matches!(error, ApplicationError::InvalidCode), "{code:?}");
        }
    }

    #[tokio::test]
    async fn a_code_github_does_not_know_is_a_status_that_does_not_echo_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": format!("Not Found: {CODE} at {CONVERSION_PATH}"),
            })))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .convert_manifest(CODE)
            .await
            .expect_err("a 404 is a failure");

        assert!(matches!(error, ApplicationError::Status(404)), "{error:?}");
        assert_echoes_nothing(&error.to_string(), &server);
    }

    #[tokio::test]
    async fn a_code_github_cannot_process_is_a_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Validation Failed",
            })))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .convert_manifest(CODE)
            .await
            .expect_err("a 422 is a failure");

        assert!(matches!(error, ApplicationError::Status(422)), "{error:?}");
        assert_echoes_nothing(&error.to_string(), &server);
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        for status in [302, 307] {
            let target = MockServer::start().await;
            Mock::given(any())
                .respond_with(ResponseTemplate::new(201).set_body_json(conversion()))
                .expect(0)
                .mount(&target)
                .await;
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("Location", format!("{}{CONVERSION_PATH}", target.uri())),
                )
                .expect(1)
                .mount(&server)
                .await;

            let error = stand_in(&server)
                .convert_manifest(CODE)
                .await
                .expect_err("a redirect must not carry the code onward");

            assert!(
                matches!(error, ApplicationError::Status(refused) if refused == status),
                "{status}: {error:?}"
            );
            assert_echoes_nothing(&error.to_string(), &server);
            target.verify().await;
        }
    }

    #[tokio::test]
    async fn an_answer_past_the_limit_is_a_response_error_that_echoes_no_credential() {
        let server = MockServer::start().await;
        let padding = " ".repeat(MAXIMUM_ANSWER_BYTES);
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(201).set_body_string(format!("{}{padding}", conversion())),
            )
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .convert_manifest(CODE)
            .await
            .expect_err("an answer past the limit is never parsed in part");

        assert!(matches!(error, ApplicationError::Response), "{error:?}");
        assert_echoes_nothing(&error.to_string(), &server);
    }

    #[tokio::test]
    async fn an_answer_missing_its_private_key_is_a_response_error_that_echoes_no_credential() {
        let server = MockServer::start().await;
        let mut answer = conversion();
        answer.as_object_mut().unwrap().remove("pem");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .convert_manifest(CODE)
            .await
            .expect_err("an App without its private key cannot sign a JWT");

        assert!(matches!(error, ApplicationError::Response), "{error:?}");
        assert_echoes_nothing(&error.to_string(), &server);
    }

    #[tokio::test]
    async fn an_answer_that_stops_arriving_is_given_up_at_the_timeout() {
        let service = stalled(
            "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"id\":",
        )
        .await;

        let converted = tokio::time::timeout(GUARD, service.convert_manifest(CODE))
            .await
            .expect("the wait for the rest of the answer is bounded by the service's timeout");

        assert!(
            matches!(converted, Err(ApplicationError::Transport(_))),
            "{converted:?}"
        );
    }

    #[tokio::test]
    async fn an_unreachable_host_surfaces_without_the_url_or_the_code() {
        let error = ApplicationService::standing_in_for("http://127.0.0.1:1")
            .unwrap()
            .convert_manifest(CODE)
            .await
            .expect_err("a closed port is a failure");

        let rendered = error.to_string();
        assert!(matches!(error, ApplicationError::Transport(_)), "{error:?}");
        assert!(
            !rendered.contains("127.0.0.1")
                && !rendered.contains("app-manifests")
                && !rendered.contains(CODE),
            "the request URL survived into the error: {rendered}"
        );
    }
}
