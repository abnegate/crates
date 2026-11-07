use crate::gitlab::GitLabError;
use crate::gitlab::GitLabResult;
use crate::gitlab::Project;
use crate::gitlab::origin::Origin;
use abnegate_http::metadata::MetadataResolver;
use abnegate_secret::SecretValue;
use reqwest::Client;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use std::time::Duration;
use url::Url;

mod blobs;
#[cfg(test)]
mod fixtures;
mod projects;
mod trees;

/// What this crate calls itself to GitLab.
const USER_AGENT: &str = "abnegate-vcs";

/// The GitLab instance addressed unless an operator configures another.
const GITLAB_URL: &str = "https://gitlab.com";

/// The header GitLab reads a personal, project or group access token from.
const PRIVATE_TOKEN: &str = "private-token";

/// Longest a request may take from sending to the last byte of its answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirects within the origin a request follows before it stops.
const MAXIMUM_REDIRECTS: usize = 10;

/// Most bytes of a JSON answer read. A longer one is refused whole, never
/// parsed in part.
const MAXIMUM_ANSWER_BYTES: usize = 16 * 1024 * 1024;

/// What an answer this crate cannot read is reported as, in place of the
/// parser's own message, which quotes the answer.
const UNREADABLE: &str = "GitLab answered in a form this crate cannot read";

/// What a JSON answer longer than [`MAXIMUM_ANSWER_BYTES`] is reported as.
const OVERSIZED: &str = "GitLab's answer was larger than this crate reads";

/// Reads a GitLab project's default branch, its tree and its blobs, on
/// gitlab.com or a self-hosted instance.
///
/// - Projects: `get_default_branch`.
/// - Trees: `fetch_tree`, which walks every page of a project's tree.
/// - Blobs: `fetch_blob`, which reads a blob's bytes up to a limit.
///
/// Every request carries the token it is given in GitLab's `PRIVATE-TOKEN`
/// header, marked sensitive so that it never appears in a debug rendering. An
/// empty token is never sent: a request given one is
/// [`GitLabError::AuthenticationFailed`], unsent, until
/// [`Self::with_anonymous_reads`] sends it without one. A token that cannot
/// be sent as a header value is [`GitLabError::AuthenticationFailed`] too,
/// unsent, and is never repeated back.
///
/// A status that says what it means on its own is reported as that: 401 as
/// [`GitLabError::AuthenticationFailed`], 403 as [`GitLabError::Forbidden`],
/// 404 as [`GitLabError::NotFound`] and 429 as [`GitLabError::RateLimited`].
/// Any other status that is not a success, a redirect that was not followed
/// among them, is [`GitLabError::Refused`] with that status. No request is
/// sent again after a rate limit; retrying is the caller's to decide.
///
/// A request is given 30 seconds from when it is sent to the last byte of its
/// answer, and 10 seconds to connect. It follows a redirect only while it
/// stays on the origin it was sent to, for at most ten hops, so the token
/// never travels to another host. It ignores any proxy the environment
/// names, and it never connects to a link-local address or a cloud
/// provider's metadata service, whatever a name resolves to; loopback, LAN
/// and private hosts stay reachable. Every JSON answer is read up to 16 MiB
/// and never parsed in part, so a longer one is [`GitLabError::GitLabApi`],
/// and one this crate cannot read is too, without quoting it.
#[derive(Debug, Clone)]
pub struct GitLabService {
    client: Client,
    origin: Origin,
    anonymous: bool,
}

impl GitLabService {
    /// Address gitlab.com.
    pub fn new() -> GitLabResult<Self> {
        Self::configured(GITLAB_URL)
    }

    /// Address the GitLab instance an operator configured: an HTTPS URL with
    /// no credentials, query or fragment, either the instance's own, such as
    /// `https://gitlab.example.com` or `https://example.com/gitlab`, or its
    /// API base, ending in `/api/v4`, with or without a trailing slash.
    ///
    /// A URL that is not one, or that names a link-local address or a cloud
    /// provider's metadata service, is [`GitLabError::InvalidOrigin`].
    /// Loopback, LAN and private hosts are accepted.
    pub fn configured(url: &str) -> GitLabResult<Self> {
        Self::addressing(Origin::configured(url, false)?)
    }

    /// Address the GitLab instance an operator configured, as
    /// [`Self::configured`] does, but permit it to be plain HTTP: for a
    /// loopback or LAN instance the operator runs.
    ///
    /// An `http://` instance is then reached over plain HTTP, and its token
    /// crosses the network in the clear, so this is never the default. An
    /// `https://` instance is addressed exactly as [`Self::configured`]
    /// would.
    pub fn configured_allowing_http(url: &str) -> GitLabResult<Self> {
        Self::addressing(Origin::configured(url, true)?)
    }

    fn addressing(origin: Origin) -> GitLabResult<Self> {
        Ok(Self {
            client: client(!origin.plain())?,
            origin,
            anonymous: false,
        })
    }

    /// Send a request given an empty token without one, so that a public
    /// project can be read with no token at all.
    ///
    /// GitLab answers it as it answers anyone, so a private project, or one
    /// that does not exist, is then [`GitLabError::NotFound`] or
    /// [`GitLabError::AuthenticationFailed`], as GitLab decides. A token
    /// that is not empty is sent as it is without this.
    ///
    /// Without it, an empty token is taken for a token that is missing by
    /// mistake, and every request given one is
    /// [`GitLabError::AuthenticationFailed`], unsent.
    #[must_use]
    pub fn with_anonymous_reads(mut self) -> Self {
        self.anonymous = true;
        self
    }

    /// The REST endpoint for `project`, followed by `rest`, each segment one
    /// percent-encoded path segment.
    fn project_endpoint(&self, project: &Project, rest: &[&str]) -> Url {
        let segments: Vec<&str> = ["projects", project.as_str()]
            .into_iter()
            .chain(rest.iter().copied())
            .collect();
        self.origin.endpoint(&segments)
    }

    /// Whether `url` is on the origin this service addresses.
    fn holds(&self, url: &Url) -> bool {
        self.origin.holds(url)
    }

    /// A GET of `url` that carries `token` unless it is empty; or, for a
    /// token that may not go, why it cannot be sent.
    fn request(&self, url: Url, token: &SecretValue) -> GitLabResult<RequestBuilder> {
        let request = self.client.get(url);
        Ok(match self.credential(token)? {
            Some(credential) => request.header(PRIVATE_TOKEN, credential),
            None => request,
        })
    }

    /// The `PRIVATE-TOKEN` header value a request given `token` carries,
    /// marked sensitive: none for an empty token under
    /// [`Self::with_anonymous_reads`], and
    /// [`GitLabError::AuthenticationFailed`] for an empty token without it
    /// or one that cannot be a header value.
    fn credential(&self, token: &SecretValue) -> GitLabResult<Option<HeaderValue>> {
        if token.is_empty() {
            return match self.anonymous {
                true => Ok(None),
                false => Err(GitLabError::AuthenticationFailed),
            };
        }
        let mut value =
            HeaderValue::from_str(token.expose()).map_err(|_| GitLabError::AuthenticationFailed)?;
        value.set_sensitive(true);
        Ok(Some(value))
    }

    /// The successful answer to `request`, or the refusal its status is.
    async fn answered(&self, request: RequestBuilder) -> GitLabResult<Response> {
        let response = request.send().await?;
        let status = response.status();
        match status.is_success() {
            true => Ok(response),
            false => Err(refusal(status)),
        }
    }

    /// GET `url` and read its answer as `T`, or as the refusal it is.
    async fn get<T: DeserializeOwned>(&self, url: Url, token: &SecretValue) -> GitLabResult<T> {
        decode(self.answered(self.request(url, token)?).await?).await
    }
}

/// A client that gives up on a request that stalls, follows redirects only
/// within the origin, ignores proxies, never connects to a link-local or
/// metadata address a name resolves to, and refuses plain HTTP unless the
/// operator opted in to it.
///
/// A redirect it will not follow comes back as the answer itself.
fn client(https_only: bool) -> GitLabResult<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .https_only(https_only)
        .no_proxy()
        .dns_resolver(MetadataResolver::new())
        .redirect(within_origin())
        .build()?)
}

/// Follow a redirect only while it stays on the origin the request was sent
/// to, and for no more than [`MAXIMUM_REDIRECTS`] hops.
fn within_origin() -> Policy {
    Policy::custom(|attempt| {
        let previous = attempt.previous();
        let within = previous
            .first()
            .is_some_and(|sent| sent.origin() == attempt.url().origin());
        if within && previous.len() <= MAXIMUM_REDIRECTS {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// What a status that is not a success means.
fn refusal(status: StatusCode) -> GitLabError {
    match status {
        StatusCode::UNAUTHORIZED => GitLabError::AuthenticationFailed,
        StatusCode::FORBIDDEN => GitLabError::Forbidden,
        StatusCode::NOT_FOUND => GitLabError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => GitLabError::RateLimited,
        _ => GitLabError::refused(status.as_u16()),
    }
}

/// A successful answer read as `T`, from no more than
/// [`MAXIMUM_ANSWER_BYTES`] of it. An answer that is longer, or is not the
/// JSON expected, is reported in fixed words, because the parser's own
/// message quotes it.
async fn decode<T: DeserializeOwned>(response: Response) -> GitLabResult<T> {
    let body = read(response, MAXIMUM_ANSWER_BYTES, || {
        GitLabError::GitLabApi(OVERSIZED.to_string())
    })
    .await?;
    serde_json::from_slice(&body).map_err(|_| GitLabError::GitLabApi(UNREADABLE.to_string()))
}

/// No more than `limit` bytes of `response`, or `oversized` for an answer
/// that declares or sends more.
async fn read(
    response: Response,
    limit: usize,
    oversized: fn() -> GitLabError,
) -> GitLabResult<Vec<u8>> {
    abnegate_http::read_capped(response, limit)
        .await
        .map_err(|error| match error {
            abnegate_http::Error::OversizedBody { .. } => oversized(),
            abnegate_http::Error::UnreadableBody(source) => GitLabError::Http(source),
            _ => GitLabError::GitLabApi(UNREADABLE.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab::service::fixtures::API;
    use crate::gitlab::service::fixtures::empty;
    use crate::gitlab::service::fixtures::served;
    use crate::gitlab::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// Where the transport tests ask.
    const ROUTE: &str = "/api/v4/projects/1";

    fn address(server: &MockServer, route: &str) -> Url {
        Url::parse(&format!("{}{route}", server.uri())).unwrap()
    }

    async fn answering(answer: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(answer)
            .mount(&server)
            .await;
        server
    }

    async fn asked(server: &MockServer, token: &SecretValue) -> GitLabResult<Value> {
        served(server).get(address(server, ROUTE), token).await
    }

    #[test]
    fn gitlab_com_is_addressed_by_default() {
        let service = GitLabService::new().unwrap();

        assert_eq!(service.origin.as_str(), "https://gitlab.com/api/v4");
        assert!(!service.origin.plain());
    }

    #[test]
    fn a_loopback_lan_or_private_instance_is_accepted() {
        for configured in [
            "https://10.0.0.5",
            "https://192.168.1.20/gitlab",
            "https://localhost",
            "https://gitlab.lan",
            "https://[::1]:8443",
        ] {
            assert!(
                GitLabService::configured(configured).is_ok(),
                "{configured}"
            );
        }
        for configured in [
            "http://127.0.0.1:8080",
            "http://gitlab.lan",
            "http://localhost:3000/api/v4",
        ] {
            assert!(
                GitLabService::configured_allowing_http(configured).is_ok(),
                "{configured}"
            );
        }
    }

    #[test]
    fn plain_http_is_refused_unless_the_operator_opts_in() {
        for configured in ["http://127.0.0.1:8080", "http://gitlab.lan"] {
            assert!(
                matches!(
                    GitLabService::configured(configured),
                    Err(GitLabError::InvalidOrigin)
                ),
                "{configured}"
            );
        }
    }

    #[test]
    fn a_link_local_or_metadata_instance_is_refused_even_over_opted_in_http() {
        for configured in [
            "http://169.254.169.254",
            "https://169.254.169.254",
            "https://metadata.google.internal",
            "http://metadata.google.internal",
            "http://[fe80::1]",
            "https://[fe80::1]",
            "http://100.100.100.200",
        ] {
            assert!(
                matches!(
                    GitLabService::configured(configured),
                    Err(GitLabError::InvalidOrigin)
                ),
                "{configured}"
            );
            assert!(
                matches!(
                    GitLabService::configured_allowing_http(configured),
                    Err(GitLabError::InvalidOrigin)
                ),
                "{configured} with plain HTTP permitted"
            );
        }
    }

    #[test]
    fn an_instance_with_credentials_a_query_or_a_fragment_is_refused() {
        for configured in [
            "https://user:token@gitlab.example.com",
            "https://gitlab.example.com/?private_token=x",
            "https://gitlab.example.com/#x",
        ] {
            assert!(
                matches!(
                    GitLabService::configured(configured),
                    Err(GitLabError::InvalidOrigin)
                ),
                "{configured}"
            );
        }
    }

    #[tokio::test]
    async fn each_status_that_says_what_it_means_is_reported_as_that() {
        for (status, expected) in [
            (
                401,
                (|failure| matches!(failure, GitLabError::AuthenticationFailed))
                    as fn(&GitLabError) -> bool,
            ),
            (403, |failure| matches!(failure, GitLabError::Forbidden)),
            (404, |failure| matches!(failure, GitLabError::NotFound)),
            (429, |failure| matches!(failure, GitLabError::RateLimited)),
            (500, |failure| {
                matches!(failure, GitLabError::Refused { status: 500 })
            }),
            (503, |failure| {
                matches!(failure, GitLabError::Refused { status: 503 })
            }),
        ] {
            let server = answering(ResponseTemplate::new(status)).await;

            let failure = asked(&server, &token()).await.unwrap_err();

            assert!(expected(&failure), "{status}: {failure:?}");
        }
    }

    #[tokio::test]
    async fn an_empty_token_without_anonymous_reads_is_refused_unsent() {
        let server = answering(ResponseTemplate::new(200).set_body_json(json!({}))).await;

        let failure = asked(&server, &empty()).await.unwrap_err();

        assert!(
            matches!(failure, GitLabError::AuthenticationFailed),
            "{failure:?}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_anonymous_read_carries_no_token() {
        let server = answering(ResponseTemplate::new(200).set_body_json(json!({}))).await;

        served(&server)
            .with_anonymous_reads()
            .get::<Value>(address(&server, ROUTE), &empty())
            .await
            .unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert!(!received[0].headers.contains_key(PRIVATE_TOKEN));
    }

    #[tokio::test]
    async fn a_token_is_sent_as_gitlab_s_private_token_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .and(header(PRIVATE_TOKEN, "token"))
            .and(header("user-agent", USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(2)
            .mount(&server)
            .await;

        for service in [served(&server), served(&server).with_anonymous_reads()] {
            service
                .get::<Value>(address(&server, ROUTE), &token())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_token_that_cannot_be_a_header_is_refused_unsent_and_never_repeated() {
        let server = answering(ResponseTemplate::new(200).set_body_json(json!({}))).await;
        let token = SecretValue::new("hunter2\nx-injected: yes");

        let failure = asked(&server, &token).await.unwrap_err();

        assert!(
            matches!(failure, GitLabError::AuthenticationFailed),
            "{failure:?}"
        );
        assert!(!failure.to_string().contains("hunter2"));
        assert!(!format!("{failure:?}").contains("hunter2"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn a_token_is_marked_sensitive() {
        let service = GitLabService::new().unwrap();

        let credential = service.credential(&token()).unwrap().unwrap();

        assert!(credential.is_sensitive());
        assert!(!format!("{credential:?}").contains("token"));
    }

    #[tokio::test]
    async fn a_redirect_within_the_origin_is_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(
                ResponseTemplate::new(301).insert_header("location", format!("{API}/projects/2")),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{API}/projects/2")))
            .and(header(PRIVATE_TOKEN, "token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "moved": true })))
            .expect(1)
            .mount(&server)
            .await;

        let answer = asked(&server, &token()).await.unwrap();

        assert_eq!(answer, json!({ "moved": true }));
    }

    #[tokio::test]
    async fn a_redirect_off_the_origin_is_refused_and_never_reached() {
        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(0)
            .mount(&elsewhere)
            .await;
        let server = answering(
            ResponseTemplate::new(302)
                .insert_header("location", address(&elsewhere, ROUTE).as_str()),
        )
        .await;

        let failure = asked(&server, &token()).await.unwrap_err();

        assert!(
            matches!(failure, GitLabError::Refused { status: 302 }),
            "{failure:?}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_answer_that_is_not_the_json_expected_is_unreadable_and_never_quoted() {
        let server = answering(ResponseTemplate::new(200).set_body_string("secret-body {")).await;

        let failure = asked(&server, &token()).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == UNREADABLE),
            "{failure:?}"
        );
        assert!(!format!("{failure} {failure:?}").contains("secret-body"));
    }

    #[tokio::test]
    async fn an_answer_longer_than_sixteen_mebibytes_is_refused_whole() {
        let mut body = vec![b' '; MAXIMUM_ANSWER_BYTES + 1];
        body[0] = b'0';
        let server = answering(ResponseTemplate::new(200).set_body_bytes(body)).await;

        let failure = asked(&server, &token()).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == OVERSIZED),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn an_answer_of_exactly_sixteen_mebibytes_is_read() {
        let mut body = vec![b' '; MAXIMUM_ANSWER_BYTES];
        body[0] = b'0';
        let server = answering(ResponseTemplate::new(200).set_body_bytes(body)).await;

        let answer = asked(&server, &token()).await.unwrap();

        assert_eq!(answer, json!(0));
    }
}
