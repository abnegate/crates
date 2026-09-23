use crate::client::HttpClient;
use crate::error::Result;
use crate::response::HttpResponse;
use async_trait::async_trait;
use reqwest::Client;
use reqwest::RequestBuilder;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The default [`HttpClient`], backed by reqwest.
///
/// [`ReqwestHttpClient::new`] builds an unguarded client. To fetch a
/// caller-supplied URL, build the client with
/// [`public_client`](crate::public_client) and convert it with `From`, so the
/// SSRF guard covers every request and every redirect hop.
#[derive(Debug, Clone)]
pub struct ReqwestHttpClient {
    client: Client,
}

impl ReqwestHttpClient {
    /// Build a client with the default timeouts.
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Client> for ReqwestHttpClient {
    fn from(client: Client) -> Self {
        Self { client }
    }
}

async fn send(request: RequestBuilder, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
    let mut request = request;
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok(HttpResponse { status, body })
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        send(self.client.get(url), headers).await
    }

    async fn post(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        send(self.client.post(url).body(body.to_string()), headers).await
    }

    async fn put(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        send(self.client.put(url).body(body.to_string()), headers).await
    }

    async fn patch(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        send(self.client.patch(url).body(body.to_string()), headers).await
    }

    async fn delete(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        send(self.client.delete(url), headers).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_string;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    #[test]
    fn the_trait_stays_usable_behind_a_pointer() {
        let client: Arc<dyn HttpClient> = Arc::new(ReqwestHttpClient::default());

        assert_eq!(Arc::strong_count(&client), 1);
    }

    #[tokio::test]
    async fn a_get_carries_its_headers_and_reads_the_body_back() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/thing"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("read"))
            .mount(&server)
            .await;

        let response = ReqwestHttpClient::new()
            .get(
                &format!("{}/thing", server.uri()),
                vec![("authorization", "Bearer token".to_string())],
            )
            .await
            .expect("the request reaches the server");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "read");
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn every_body_carrying_method_sends_its_body() {
        for (verb, expected) in [("POST", 201), ("PUT", 200), ("PATCH", 200)] {
            let server = MockServer::start().await;
            Mock::given(method(verb))
                .and(path("/thing"))
                .and(body_string("written"))
                .respond_with(ResponseTemplate::new(expected))
                .mount(&server)
                .await;

            let client = ReqwestHttpClient::new();
            let url = format!("{}/thing", server.uri());
            let response = match verb {
                "POST" => client.post(&url, Vec::new(), "written").await,
                "PUT" => client.put(&url, Vec::new(), "written").await,
                _ => client.patch(&url, Vec::new(), "written").await,
            }
            .expect("the request reaches the server");

            assert_eq!(response.status, expected, "{verb} answered wrongly");
        }
    }

    #[tokio::test]
    async fn a_delete_reports_the_status_it_was_given() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/thing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let response = ReqwestHttpClient::new()
            .delete(&format!("{}/thing", server.uri()), Vec::new())
            .await
            .expect("the request reaches the server");

        assert!(response.is_not_found());
    }

    #[tokio::test]
    async fn an_unreachable_host_is_a_transport_error() {
        let error = ReqwestHttpClient::new()
            .get("http://127.0.0.1:1/", Vec::new())
            .await
            .expect_err("nothing listens there");

        assert!(matches!(error, crate::HttpError::Request(_)), "{error}");
    }
}
