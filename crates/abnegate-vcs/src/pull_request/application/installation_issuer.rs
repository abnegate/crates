use crate::pull_request::application::API_VERSION;
use crate::pull_request::application::API_VERSION_HEADER;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::InstallationId;
use crate::pull_request::application::InstallationToken;
use crate::pull_request::application::service::answered;
use crate::pull_request::origin::Origin;
use crate::pull_request::service::ACCEPT;
use crate::pull_request::service::GITHUB_API_URL;
use crate::pull_request::service::REQUEST_TIMEOUT;
use crate::pull_request::service::client;
use abnegate_secret::SecretValue;
use reqwest::Client;
use reqwest::header;
use reqwest::redirect::Policy;
use std::time::Duration;
use url::Url;

/// Trades a GitHub App's JWT for an installation access token.
///
/// It follows no redirect, since a redirect is the one way the JWT in a
/// request's `Authorization` header reaches a host GitHub did not choose: a
/// redirect is [`ApplicationError::Status`] like any other refusal. A
/// refusal keeps its status and nothing else, since the answer to a refused
/// authentication routinely quotes what was sent. A request is given 30
/// seconds from sending to the last byte of its answer.
///
/// An answer is read up to the 16 MiB this crate reads of any JSON answer,
/// which holds the metadata of the 500 repositories a token can be scoped
/// to many times over; a longer one is [`ApplicationError::Response`], never
/// parsed in part.
#[derive(Debug, Clone)]
pub struct InstallationIssuer {
    client: Client,
    origin: Origin,
}

impl InstallationIssuer {
    /// Ask GitHub's own API.
    pub fn new() -> Result<Self, ApplicationError> {
        Self::configured(GITHUB_API_URL)
    }

    /// Ask the origin an operator configured, such as a GitHub Enterprise
    /// install's: an HTTPS URL with nothing but a host and a path, with or
    /// without a trailing slash.
    pub fn configured(url: &str) -> Result<Self, ApplicationError> {
        let origin = Origin::configured(url, false).map_err(|_| ApplicationError::InvalidOrigin)?;
        Self::addressing(origin, true, REQUEST_TIMEOUT)
    }

    /// Ask the origin an operator configured, as [`Self::configured`] does,
    /// or a plain HTTP one, such as a GitHub Enterprise install on a private
    /// network, which is then reached over plain HTTP. An HTTPS origin is
    /// still reached over HTTPS alone. Over plain HTTP every JWT sent, and
    /// every installation token issued, cross the network unencrypted.
    pub fn configured_allowing_http(url: &str) -> Result<Self, ApplicationError> {
        let origin = Origin::configured(url, true).map_err(|_| ApplicationError::InvalidOrigin)?;
        let https_only = !origin.plain();
        Self::addressing(origin, https_only, REQUEST_TIMEOUT)
    }

    /// Ask `url`, over whatever scheme it names.
    ///
    /// No operator setting produces one: a configured origin is reached over
    /// HTTPS unless plain HTTP was allowed. Tests use this to drive the real
    /// request path against a mock server.
    #[cfg(any(test, feature = "testing"))]
    pub fn standing_in_for(url: &str) -> Result<Self, ApplicationError> {
        let origin =
            Origin::standing_in_for("", url).map_err(|_| ApplicationError::InvalidOrigin)?;
        Self::addressing(origin, false, REQUEST_TIMEOUT)
    }

    /// Ask `origin` with a client that follows no redirect and gives a
    /// request up after `timeout`, from sending to the last byte of its
    /// answer.
    fn addressing(
        origin: Origin,
        https_only: bool,
        timeout: Duration,
    ) -> Result<Self, ApplicationError> {
        let client =
            client(https_only, timeout, Policy::none()).map_err(|_| ApplicationError::Client)?;
        Ok(Self { client, origin })
    }

    /// Trade `jwt`, from [`crate::pull_request::ApplicationClaims::sign`], for
    /// a token scoped to `installation`. An empty `jwt` is
    /// [`ApplicationError::AuthenticationRequired`], with nothing sent.
    pub async fn issue(
        &self,
        jwt: &SecretValue,
        installation: InstallationId,
    ) -> Result<InstallationToken, ApplicationError> {
        if jwt.is_empty() {
            return Err(ApplicationError::AuthenticationRequired);
        }
        let request = self
            .client
            .post(self.endpoint(installation)?)
            .header(header::ACCEPT, ACCEPT)
            .header(API_VERSION_HEADER, API_VERSION)
            .bearer_auth(jwt.expose());
        let body = answered(request).await?;
        InstallationToken::parse(&body)
    }

    /// Where the token for `installation` is asked for.
    fn endpoint(&self, installation: InstallationId) -> Result<Url, ApplicationError> {
        let installation = installation.get().to_string();
        self.origin
            .endpoint(&["app", "installations", &installation, "access_tokens"])
            .map_err(|_| ApplicationError::InvalidOrigin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::fixtures::GUARD;
    use crate::pull_request::application::fixtures::SHORT_TIMEOUT;
    use crate::pull_request::application::fixtures::stalling;
    use crate::pull_request::service::MAXIMUM_ANSWER_BYTES;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::header;
    use wiremock::matchers::header_exists;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// The path the token for installation 67890 is asked for at.
    const TOKEN_PATH: &str = "/app/installations/67890/access_tokens";

    fn jwt() -> SecretValue {
        SecretValue::new("header.payload.signature")
    }

    fn installation() -> InstallationId {
        InstallationId::new(67890)
    }

    fn stand_in(server: &MockServer) -> InstallationIssuer {
        InstallationIssuer::standing_in_for(&server.uri()).unwrap()
    }

    fn issued() -> ResponseTemplate {
        ResponseTemplate::new(201).set_body_json(json!({
            "token": "ghs_installationtokenvalue",
            "expires_at": "2026-01-15T12:00:00Z",
        }))
    }

    #[test]
    fn the_token_url_names_the_installation() {
        for (configured, expected) in [
            (
                "https://github.example.com/api/v3",
                "https://github.example.com/api/v3/app/installations/67890/access_tokens",
            ),
            (
                "https://github.example.com/api/v3/",
                "https://github.example.com/api/v3/app/installations/67890/access_tokens",
            ),
        ] {
            assert_eq!(
                InstallationIssuer::configured(configured)
                    .unwrap()
                    .endpoint(installation())
                    .unwrap()
                    .as_str(),
                expected,
                "{configured}"
            );
        }
        assert_eq!(
            InstallationIssuer::new()
                .unwrap()
                .endpoint(installation())
                .unwrap()
                .as_str(),
            "https://api.github.com/app/installations/67890/access_tokens"
        );
    }

    #[test]
    fn a_plain_http_origin_names_the_installation_once_the_operator_allows_it() {
        assert_eq!(
            InstallationIssuer::configured_allowing_http("http://192.168.1.5/api/v3")
                .unwrap()
                .endpoint(installation())
                .unwrap()
                .as_str(),
            "http://192.168.1.5/api/v3/app/installations/67890/access_tokens"
        );
    }

    #[test]
    fn allowing_plain_http_admits_no_other_scheme_or_shape() {
        for refused in [
            "ftp://github.example.com",
            "http://user:token@192.168.1.5",
            "http://192.168.1.5/?token=x",
            "http://192.168.1.5/#x",
        ] {
            assert!(
                matches!(
                    InstallationIssuer::configured_allowing_http(refused),
                    Err(ApplicationError::InvalidOrigin)
                ),
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn a_plain_http_origin_the_operator_allowed_is_reached_over_plain_http() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(issued())
            .expect(1)
            .mount(&server)
            .await;

        let token = InstallationIssuer::configured_allowing_http(&server.uri())
            .unwrap()
            .issue(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(token.secret().expose(), "ghs_installationtokenvalue");
    }

    #[test]
    fn a_configured_origin_must_be_https_with_nothing_but_a_host_and_a_path() {
        for refused in [
            "http://github.example.com/api/v3",
            "github.example.com",
            "https://user:token@github.example.com",
            "https://github.example.com/?token=x",
        ] {
            assert!(
                matches!(
                    InstallationIssuer::configured(refused),
                    Err(ApplicationError::InvalidOrigin)
                ),
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn a_successful_exchange_yields_a_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .and(header("Authorization", "Bearer header.payload.signature"))
            .and(header(API_VERSION_HEADER, API_VERSION))
            .and(header_exists("Accept"))
            .respond_with(issued())
            .expect(1)
            .mount(&server)
            .await;

        let token = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(token.secret().expose(), "ghs_installationtokenvalue");
    }

    #[tokio::test]
    async fn a_refusal_surfaces_as_a_status_without_the_url_or_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "Bad credentials: header.payload.signature",
            })))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .expect_err("a 401 is a failure");

        let rendered = error.to_string();
        assert!(matches!(error, ApplicationError::Status(401)));
        assert!(!rendered.contains("header.payload.signature"));
        assert!(!rendered.contains("access_tokens"));
        assert!(!rendered.contains(&server.uri()));
    }

    #[tokio::test]
    async fn an_unreachable_host_surfaces_without_the_credentialed_url() {
        let error = InstallationIssuer::standing_in_for("http://127.0.0.1:1")
            .unwrap()
            .issue(&jwt(), installation())
            .await
            .expect_err("a closed port is a failure");

        let rendered = error.to_string();
        assert!(matches!(error, ApplicationError::Transport(_)));
        assert!(
            !rendered.contains("127.0.0.1") && !rendered.contains("access_tokens"),
            "the request URL survived into the error: {rendered}"
        );
        assert!(!rendered.contains("header.payload.signature"));
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        let target = MockServer::start().await;
        Mock::given(any())
            .respond_with(issued())
            .expect(0)
            .mount(&target)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}{TOKEN_PATH}", target.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .expect_err("a redirect must not carry the JWT onward");

        assert!(matches!(error, ApplicationError::Status(302)));
        target.verify().await;
    }

    /// How many repositories GitHub lets a token request name.
    const SCOPED_REPOSITORIES: usize = 500;

    /// About as many bytes as GitHub spends describing one repository.
    const REPOSITORY_BYTES: usize = 7 * 1024;

    #[tokio::test]
    async fn an_answer_listing_every_repository_a_token_is_scoped_to_yields_a_token() {
        let server = MockServer::start().await;
        let repositories: Vec<serde_json::Value> = (1..=SCOPED_REPOSITORIES)
            .map(|id| {
                json!({
                    "id": id,
                    "full_name": format!("acme/repository-{id}"),
                    "description": "d".repeat(REPOSITORY_BYTES),
                })
            })
            .collect();
        let body = json!({
            "token": "ghs_installationtokenvalue",
            "expires_at": "2026-01-15T12:00:00Z",
            "repository_selection": "selected",
            "repositories": repositories,
        });
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(ResponseTemplate::new(201).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;

        let token = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .unwrap();

        assert_eq!(token.secret().expose(), "ghs_installationtokenvalue");
    }

    /// The start of a token answer that promises more than it sends.
    const STALLED: &str = "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"token\":";

    #[tokio::test]
    async fn an_answer_that_stops_arriving_is_given_up_at_the_timeout() {
        let origin = Origin::standing_in_for("", &stalling(STALLED).await).unwrap();
        let issuer = InstallationIssuer::addressing(origin, false, SHORT_TIMEOUT).unwrap();

        let issued = tokio::time::timeout(GUARD, issuer.issue(&jwt(), installation()))
            .await
            .expect("the body read is bounded by the issuer's timeout");

        assert!(
            matches!(issued, Err(ApplicationError::Transport(_))),
            "{issued:?}"
        );
    }

    #[tokio::test]
    async fn an_answer_past_the_byte_limit_is_refused_whole() {
        let server = MockServer::start().await;
        let padding = " ".repeat(MAXIMUM_ANSWER_BYTES);
        let body = format!(
            r#"{{"token":"ghs_installationtokenvalue","expires_at":"2026-01-15T12:00:00Z"}}{padding}"#
        );
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_string(body))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .expect_err("an answer past the limit is never parsed in part");

        assert!(matches!(error, ApplicationError::Response));
        assert!(!error.to_string().contains("ghs_installationtokenvalue"));
    }

    #[tokio::test]
    async fn an_unreadable_answer_is_a_response_error_that_does_not_echo_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"token":"ghs_leaky"}"#))
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .issue(&jwt(), installation())
            .await
            .expect_err("an answer without an expiry is a failure");

        assert!(matches!(error, ApplicationError::Response));
        assert!(!error.to_string().contains("ghs_leaky"));
    }

    #[tokio::test]
    async fn an_empty_jwt_is_refused_unsent() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(issued())
            .expect(0)
            .mount(&server)
            .await;

        let error = stand_in(&server)
            .issue(&SecretValue::new(""), installation())
            .await
            .expect_err("an empty JWT is never sent");

        assert!(
            matches!(error, ApplicationError::AuthenticationRequired),
            "{error:?}"
        );
    }
}
