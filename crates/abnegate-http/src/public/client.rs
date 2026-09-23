use crate::error::Result;
use crate::public::request::PublicRequest;
use crate::url::validate_public_url;
use reqwest::Client;
use reqwest::Method;
use reqwest::Request;
use reqwest::RequestBuilder;
use reqwest::Response;

/// A reqwest client for fetching caller-supplied URLs without reaching back
/// inside the deployment.
///
/// Every request is checked by [`validate_public_url`] before it is started
/// and again before it is sent, so an IP literal, which reqwest connects to
/// without consulting any resolver, is refused before a connection is
/// attempted. Every name the client resolves is refused if it answers only
/// with addresses that must not be fetched, and every redirect hop is checked
/// again before it is followed. The reqwest client underneath is never handed
/// out, not even through the [`PublicRequest`] a request method returns.
///
/// Build one with [`public_client`](crate::public_client) or
/// [`public_client_builder`](crate::public_client_builder).
///
/// ```
/// use abnegate_http::{HttpError, public_client};
/// use std::time::Duration;
///
/// let client = public_client(Duration::from_secs(10))?;
///
/// assert!(matches!(
///     client.get("http://127.0.0.1:8080/admin"),
///     Err(HttpError::PrivateAddress)
/// ));
/// assert!(client.get("https://example.com/").is_ok());
/// # Ok::<(), HttpError>(())
/// ```
#[derive(Debug, Clone)]
pub struct PublicClient {
    client: Client,
}

impl PublicClient {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Start a `method` request to `url`, refusing a URL that must not be
    /// fetched before anything is sent.
    pub fn request(&self, method: Method, url: &str) -> Result<PublicRequest> {
        self.prepare(method, url).map(PublicRequest::new)
    }

    /// Start a `method` request to `url` as a reqwest builder, which carries
    /// the client underneath and so must not leave this crate.
    pub(crate) fn prepare(&self, method: Method, url: &str) -> Result<RequestBuilder> {
        Ok(self.client.request(method, validate_public_url(url)?))
    }

    /// Start a GET request to `url`.
    pub fn get(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::GET, url)
    }

    /// Start a HEAD request to `url`.
    pub fn head(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::HEAD, url)
    }

    /// Start a POST request to `url`.
    pub fn post(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::POST, url)
    }

    /// Start a PUT request to `url`.
    pub fn put(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::PUT, url)
    }

    /// Start a PATCH request to `url`.
    pub fn patch(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::PATCH, url)
    }

    /// Start a DELETE request to `url`.
    pub fn delete(&self, url: &str) -> Result<PublicRequest> {
        self.request(Method::DELETE, url)
    }

    /// Send a request built elsewhere, refusing its URL first if it must not
    /// be fetched.
    pub async fn execute(&self, request: Request) -> Result<Response> {
        validate_public_url(request.url().as_str())?;
        Ok(self.client.execute(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use crate::error::HttpError;
    use crate::public::public_client;
    use crate::test_support::LOOPBACK_SPELLINGS;
    use crate::test_support::assert_untouched;
    use crate::test_support::loopback_listener;
    use reqwest::Method;
    use reqwest::Request;
    use reqwest::Url;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn every_spelling_of_loopback_is_refused_before_a_connection_is_made() {
        let client = public_client(TIMEOUT).expect("a guarded client");

        for spelling in LOOPBACK_SPELLINGS {
            let (listener, port) = loopback_listener();
            let url = format!("http://{spelling}:{port}/");

            for method in [
                Method::GET,
                Method::HEAD,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ] {
                let error = client
                    .request(method.clone(), &url)
                    .expect_err("loopback must not be fetched");
                assert!(
                    matches!(error, HttpError::PrivateAddress),
                    "{method} {url}: {error}"
                );
            }
            assert_untouched(&listener);
        }
    }

    #[tokio::test]
    async fn a_prebuilt_request_to_loopback_is_refused_before_a_connection_is_made() {
        let client = public_client(TIMEOUT).expect("a guarded client");

        for spelling in LOOPBACK_SPELLINGS {
            let (listener, port) = loopback_listener();
            let url = Url::parse(&format!("http://{spelling}:{port}/")).expect("a URL");

            let error = client
                .execute(Request::new(Method::GET, url))
                .await
                .expect_err("loopback must not be fetched");

            assert!(matches!(error, HttpError::PrivateAddress), "{error}");
            assert_untouched(&listener);
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_loopback_is_refused_by_the_resolver() {
        let client = public_client(TIMEOUT).expect("a guarded client");
        let (listener, port) = loopback_listener();

        let error = client
            .get(&format!("http://localhost.:{port}/"))
            .expect_err("localhost is an internal name");

        assert!(matches!(error, HttpError::InternalHost), "{error}");
        assert_untouched(&listener);
    }

    #[test]
    fn a_public_url_starts_a_request() {
        let client = public_client(TIMEOUT).expect("a guarded client");

        let request = client
            .post("https://example.com/submit?page=2")
            .expect("a public URL is fetchable")
            .build()
            .expect("the request builds");

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.url().as_str(), "https://example.com/submit?page=2");
    }
}
