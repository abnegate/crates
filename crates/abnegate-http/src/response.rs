use crate::error::Result;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

/// An HTTP response reduced to the parts a caller reads, so a source adapter or
/// an API client can be unit tested without a transport behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The response status code.
    pub status: u16,
    /// The response body, decoded as text.
    pub body: String,
}

impl HttpResponse {
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
    fn http_response_success_200() {
        let response = HttpResponse {
            status: 200,
            body: String::new(),
        };
        assert!(response.is_success());
    }

    #[test]
    fn http_response_success_299() {
        let response = HttpResponse {
            status: 299,
            body: String::new(),
        };
        assert!(response.is_success());
    }

    #[test]
    fn http_response_failure_400() {
        let response = HttpResponse {
            status: 400,
            body: String::new(),
        };
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_failure_500() {
        let response = HttpResponse {
            status: 500,
            body: String::new(),
        };
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_not_found() {
        let response = HttpResponse {
            status: 404,
            body: String::new(),
        };
        assert!(response.is_not_found());
        assert!(!response.is_success());
    }

    #[test]
    fn http_response_json_valid() {
        let response = HttpResponse {
            status: 200,
            body: r#"{"key": "value"}"#.to_string(),
        };
        let parsed: HashMap<String, String> = response.json().expect("valid JSON");
        assert_eq!(parsed.get("key").map(String::as_str), Some("value"));
    }

    #[test]
    fn http_response_json_invalid() {
        let response = HttpResponse {
            status: 200,
            body: "not json".to_string(),
        };
        let parsed: Result<serde_json::Value> = response.json();
        assert!(parsed.is_err());
    }
}
