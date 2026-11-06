use crate::pull_request::PullRequestError;
use crate::pull_request::application::API_VERSION;
use crate::pull_request::application::API_VERSION_HEADER;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::origin::Origin;
use crate::pull_request::service::ACCEPT;
use crate::pull_request::service::GITHUB_API_URL;
use crate::pull_request::service::MAXIMUM_ANSWER_BYTES;
use crate::pull_request::service::REQUEST_TIMEOUT;
use crate::pull_request::service::client;
use crate::pull_request::service::read_prefix;
use abnegate_secret::SecretValue;
use reqwest::Client;
use reqwest::Method;
use reqwest::RequestBuilder;
use reqwest::StatusCode;
use reqwest::header;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use std::time::Duration;
use url::Url;

#[cfg(test)]
mod fixtures;
mod installations;
mod manifests;
mod repositories;

/// Asks GitHub's REST API about a GitHub App: the App a manifest registers,
/// the installations it has, and the repositories each installation reaches.
///
/// - [`Self::convert_manifest`] registers an App from the code GitHub hands
///   back once someone creates it from a manifest, and needs no credential.
/// - [`Self::fetch_installation`] reads one installation, with the App's JWT.
/// - [`Self::list_installations`] reads a page of the App's installations,
///   with the App's JWT.
/// - [`Self::delete_installation`] uninstalls the App from one installation,
///   with the App's JWT.
/// - [`Self::list_installation_repositories`] reads a page of the
///   repositories an installation reaches, with that installation's token.
///
/// It follows no redirect, since a redirect is the one way the JWT or token
/// in a request's `Authorization` header reaches a host GitHub did not
/// choose: a redirect is [`ApplicationError::Status`] like any other
/// refusal. A refusal keeps its status and nothing else, since the answer to
/// a refused request routinely quotes what was sent. A request is given 30
/// seconds from sending to the last byte of its answer. An empty JWT or
/// token is [`ApplicationError::AuthenticationRequired`], never sent.
///
/// An answer is read up to the 16 MiB this crate reads of any JSON answer; a
/// longer one is [`ApplicationError::Response`], never parsed in part.
#[derive(Debug, Clone)]
pub struct ApplicationService {
    client: Client,
    origin: Origin,
}

impl ApplicationService {
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
    /// still reached over HTTPS alone. Over plain HTTP every JWT and token
    /// sent, and the private key and secrets a manifest conversion answers
    /// with, cross the network unencrypted.
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

    /// The origin's URL with `segments` appended, each percent-encoded as one
    /// path segment. A segment that is empty, `.` or `..` is
    /// [`ApplicationError::InvalidOrigin`], since it would reach another
    /// endpoint; the fixed names and numeric ids every caller passes never
    /// are.
    fn endpoint(&self, segments: &[&str]) -> Result<Url, ApplicationError> {
        self.origin
            .endpoint(segments)
            .map_err(|_| ApplicationError::InvalidOrigin)
    }

    /// A `method` request to `url` that asks for GitHub's REST media type
    /// under [`API_VERSION`], carrying no credential.
    fn request(&self, method: Method, url: Url) -> RequestBuilder {
        self.client
            .request(method, url)
            .header(header::ACCEPT, ACCEPT)
            .header(API_VERSION_HEADER, API_VERSION)
    }

    /// [`Self::request`], carrying `credential`, the App's JWT or an
    /// installation's token, as a bearer credential. An empty `credential`
    /// is [`ApplicationError::AuthenticationRequired`], with nothing sent.
    fn authorized(
        &self,
        method: Method,
        url: Url,
        credential: &SecretValue,
    ) -> Result<RequestBuilder, ApplicationError> {
        if credential.is_empty() {
            return Err(ApplicationError::AuthenticationRequired);
        }
        Ok(self.request(method, url).bearer_auth(credential.expose()))
    }
}

/// The body of GitHub's answer to `request`, sent.
///
/// A request that does not complete is [`ApplicationError::Transport`],
/// without the URL it was sent to. An answer that is not a success, a
/// redirect included, is [`ApplicationError::Status`], its body unread. A
/// body that stops arriving is [`ApplicationError::Transport`], and one
/// longer than [`MAXIMUM_ANSWER_BYTES`] or otherwise unreadable is
/// [`ApplicationError::Response`].
pub(super) async fn answered(request: RequestBuilder) -> Result<Vec<u8>, ApplicationError> {
    let response = request.send().await.map_err(ApplicationError::transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApplicationError::Status(status.as_u16()));
    }
    let (body, truncated) = read_prefix(response, MAXIMUM_ANSWER_BYTES)
        .await
        .map_err(|error| match error {
            PullRequestError::Http(error) => ApplicationError::transport(error),
            _ => ApplicationError::Response,
        })?;
    if truncated {
        return Err(ApplicationError::Response);
    }
    Ok(body)
}

/// `body` read as JSON. Any failure is [`ApplicationError::Response`] alone,
/// since the parser's own message quotes the body, which may hold a token, a
/// secret or a private key.
pub(super) fn parsed<T: DeserializeOwned>(body: &[u8]) -> Result<T, ApplicationError> {
    serde_json::from_slice(body).map_err(|_| ApplicationError::Response)
}

/// Whether GitHub carried out `request`, a delete, sent: a success, or a
/// `404` for what is already gone, is done. Anything else, a redirect
/// included, is [`ApplicationError::Status`], and the body is never read.
async fn acknowledged(request: RequestBuilder) -> Result<(), ApplicationError> {
    let response = request.send().await.map_err(ApplicationError::transport)?;
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_FOUND {
        return Ok(());
    }
    Err(ApplicationError::Status(status.as_u16()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::service::fixtures::installation;
    use crate::pull_request::application::service::fixtures::jwt;
    use crate::pull_request::application::service::fixtures::stand_in;
    use std::collections::BTreeMap;
    use std::num::NonZeroU8;
    use std::num::NonZeroU32;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// Where installation 67890 is addressed under any origin.
    fn installation_endpoint(service: &ApplicationService) -> String {
        let installation = installation().get().to_string();
        service
            .endpoint(&["app", "installations", &installation])
            .unwrap()
            .to_string()
    }

    #[test]
    fn githubs_own_api_is_asked_by_default() {
        assert_eq!(
            installation_endpoint(&ApplicationService::new().unwrap()),
            "https://api.github.com/app/installations/67890"
        );
    }

    #[test]
    fn a_configured_origin_is_asked_with_or_without_a_trailing_slash() {
        for configured in [
            "https://github.example.com/api/v3",
            "https://github.example.com/api/v3/",
        ] {
            assert_eq!(
                installation_endpoint(&ApplicationService::configured(configured).unwrap()),
                "https://github.example.com/api/v3/app/installations/67890",
                "{configured}"
            );
        }
    }

    #[test]
    fn a_configured_origin_must_be_https_with_nothing_but_a_host_and_a_path() {
        for refused in [
            "http://github.example.com/api/v3",
            "github.example.com",
            "https://user:token@github.example.com",
            "https://github.example.com/?token=x",
            "https://github.example.com/#x",
        ] {
            assert!(
                matches!(
                    ApplicationService::configured(refused),
                    Err(ApplicationError::InvalidOrigin)
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_plain_http_origin_is_asked_once_the_operator_allows_it() {
        for (configured, expected) in [
            (
                "http://192.168.1.5/api/v3",
                "http://192.168.1.5/api/v3/app/installations/67890",
            ),
            (
                "http://localhost:3000",
                "http://localhost:3000/app/installations/67890",
            ),
            (
                "https://github.example.com/api/v3",
                "https://github.example.com/api/v3/app/installations/67890",
            ),
        ] {
            assert_eq!(
                installation_endpoint(
                    &ApplicationService::configured_allowing_http(configured).unwrap()
                ),
                expected,
                "{configured}"
            );
        }
    }

    #[test]
    fn allowing_plain_http_admits_no_other_scheme_or_shape() {
        for refused in [
            "ftp://github.example.com",
            "localhost:3000",
            "http://user:token@192.168.1.5",
            "http://192.168.1.5/?token=x",
            "http://192.168.1.5/#x",
        ] {
            assert!(
                matches!(
                    ApplicationService::configured_allowing_http(refused),
                    Err(ApplicationError::InvalidOrigin)
                ),
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn a_plain_http_origin_the_operator_allowed_is_reached_over_plain_http() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/app/installations/67890"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        ApplicationService::configured_allowing_http(&server.uri())
            .unwrap()
            .delete_installation(&jwt(), installation())
            .await
            .unwrap();
    }

    #[test]
    fn an_endpoint_refuses_a_segment_that_would_reach_another_endpoint() {
        let service = ApplicationService::new().unwrap();
        for refused in ["", ".", ".."] {
            assert!(
                matches!(
                    service.endpoint(&["app", "installations", refused]),
                    Err(ApplicationError::InvalidOrigin)
                ),
                "{refused:?}"
            );
        }
        assert_eq!(
            service
                .endpoint(&["app", "installations", "a/b"])
                .unwrap()
                .as_str(),
            "https://api.github.com/app/installations/a%2Fb"
        );
    }

    #[test]
    fn a_service_renders_its_origin_and_never_a_credential() {
        let rendered = format!(
            "{:?}",
            ApplicationService::configured("https://github.example.com/api/v3").unwrap()
        );

        assert!(rendered.contains("github.example.com"), "{rendered}");
        assert!(
            !rendered.contains('@') && !rendered.contains("token"),
            "{rendered}"
        );
        for credentialed in [
            "https://user:token@github.example.com",
            "https://github.example.com/?token=x",
            "https://github.example.com/#token",
        ] {
            assert!(
                ApplicationService::configured(credentialed).is_err(),
                "an origin carrying a credential would render it: {credentialed}"
            );
        }
    }

    #[test]
    fn an_unreadable_body_is_a_response_error_that_does_not_echo_it() {
        for body in [
            br#"{"token":"ghs_leaky""#.as_slice(),
            br#"["ghs_leaky"]"#,
            b"",
        ] {
            let error =
                parsed::<BTreeMap<String, String>>(body).expect_err("an unreadable body fails");

            assert!(matches!(error, ApplicationError::Response));
            assert!(!error.to_string().contains("ghs_leaky"));
        }
    }

    #[tokio::test]
    async fn an_empty_credential_is_refused_unsent() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let service = stand_in(&server);
        let empty = SecretValue::new("");
        let first = NonZeroU32::MIN;
        let per_page = NonZeroU8::MIN;

        let refusals = [
            service
                .fetch_installation(&empty, installation())
                .await
                .map(drop),
            service
                .list_installations(&empty, first, per_page)
                .await
                .map(drop),
            service.delete_installation(&empty, installation()).await,
            service
                .list_installation_repositories(&empty, first, per_page)
                .await
                .map(drop),
        ];

        for refusal in refusals {
            assert!(
                matches!(refusal, Err(ApplicationError::AuthenticationRequired)),
                "{refusal:?}"
            );
        }
    }
}
