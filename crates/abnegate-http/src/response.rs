use crate::error::Result;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

/// An HTTP response reduced to the parts a caller reads, so a source adapter or
/// an API client can be unit tested without a transport behind it.
///
/// A field may be added in a minor release, so an [`HttpClient`] implemented
/// outside this crate answers with [`HttpResponse::new`] rather than a literal:
///
/// ```
/// use abnegate_http::HttpResponse;
///
/// let response = HttpResponse::new(404, "gone");
///
/// assert!(response.is_not_found());
/// assert_eq!(response.body, "gone");
/// ```
///
/// ```compile_fail,E0639
/// use abnegate_http::HttpResponse;
///
/// let _ = HttpResponse {
///     status: 200,
///     body: String::new(),
/// };
/// ```
///
/// [`HttpClient`]: crate::HttpClient
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpResponse {
    /// The response status code.
    pub status: u16,
    /// The response body, decoded as UTF-8 with invalid sequences replaced.
    pub body: String,
}

impl HttpResponse {
    /// A response with `status` and `body`.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// Whether the status is a success (2xx).
    pub fn is_success(&self) -> bool {
        StatusCode::from_u16(self.status).is_ok_and(|status| status.is_success())
    }

    /// Whether the status is 404 Not Found.
    pub fn is_not_found(&self) -> bool {
        self.status == StatusCode::NOT_FOUND.as_u16()
    }

    /// Parse the body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_str(&self.body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_response_holds_the_status_and_body_it_was_built_with() {
        let response = HttpResponse::new(201, String::from("made"));

        assert_eq!(response.status, 201);
        assert_eq!(response.body, "made");
    }

    #[test]
    fn http_response_success_200() {
        let response = HttpResponse::new(200, "");
        assert!(response.is_success());
    }

    #[test]
    fn http_response_success_299() {
        let response = HttpResponse::new(299, "");
        assert!(response.is_success());
    }

    #[test]
    fn http_response_failure_400() {
        let response = HttpResponse::new(400, "");
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_failure_500() {
        let response = HttpResponse::new(500, "");
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_not_found() {
        let response = HttpResponse::new(404, "");
        assert!(response.is_not_found());
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_json_valid() {
        let response = HttpResponse::new(200, r#"{"key": "value"}"#);
        let parsed: HashMap<String, String> = response.json().expect("valid JSON");
        assert_eq!(parsed.get("key").map(String::as_str), Some("value"));
    }

    #[test]
    fn http_response_json_invalid() {
        let response = HttpResponse::new(200, "not json");
        let parsed: Result<serde_json::Value> = response.json();
        assert!(parsed.is_err());
    }
}
