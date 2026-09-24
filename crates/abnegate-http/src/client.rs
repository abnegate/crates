use crate::error::Error;
use crate::error::Result;
use crate::response::HttpResponse;
use async_trait::async_trait;
use reqwest::Method;

/// The HTTP operations a caller needs, behind a trait so a test can stand in
/// for the network.
///
/// Only [`HttpClient::get`] has to be implemented; every other method reports
/// [`Error::Unsupported`] until an implementation overrides it.
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Perform a GET request with headers.
    async fn get(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse>;

    /// Perform a POST request with headers and a body.
    async fn post(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        let _ = (url, headers, body);
        Err(Error::Unsupported(Method::POST))
    }

    /// Perform a PUT request with headers and a body.
    async fn put(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        let _ = (url, headers, body);
        Err(Error::Unsupported(Method::PUT))
    }

    /// Perform a PATCH request with headers and a body.
    async fn patch(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        let _ = (url, headers, body);
        Err(Error::Unsupported(Method::PATCH))
    }

    /// Perform a DELETE request with headers.
    async fn delete(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        let _ = (url, headers);
        Err(Error::Unsupported(Method::DELETE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct GetOnly;

    #[async_trait]
    impl HttpClient for GetOnly {
        async fn get(&self, url: &str, _headers: Vec<(&str, String)>) -> Result<HttpResponse> {
            Ok(HttpResponse::new(200, url))
        }
    }

    #[tokio::test]
    async fn an_implementation_only_has_to_answer_get() {
        let client = GetOnly;

        let response = client
            .get("https://example.com/", Vec::new())
            .await
            .expect("GET is implemented");
        assert_eq!(response.body, "https://example.com/");
    }

    #[tokio::test]
    async fn every_other_method_reports_which_one_is_missing() {
        let client = GetOnly;

        for error in [
            client.post("https://example.com/", Vec::new(), "").await,
            client.put("https://example.com/", Vec::new(), "").await,
            client.patch("https://example.com/", Vec::new(), "").await,
            client.delete("https://example.com/", Vec::new()).await,
        ] {
            let error = error.expect_err("the default body refuses");
            assert!(
                error
                    .to_string()
                    .ends_with("is not supported by this HTTP client"),
                "{error}"
            );
        }
    }
}
