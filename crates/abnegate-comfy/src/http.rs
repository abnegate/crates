//! The HTTP client every ComfyUI request goes through.

use crate::config::Config;
use crate::config::MINIMUM_TIMEOUT_SECONDS;
use reqwest::RequestBuilder;
use reqwest::redirect::Policy;
use std::time::Duration;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling on one history poll, so a stalled poll cannot eat the deadline it
/// is polling against.
pub(crate) const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an abandoned prompt is given to reach terminal history.
pub(crate) const CANCEL_TIMEOUT: Duration = Duration::from_secs(30);

/// A client that never follows a redirect, so the token header cannot be
/// carried to whatever host a response points at.
pub(crate) fn client(config: &Config) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout(config))
        .build()
}

pub(crate) fn request_timeout(config: &Config) -> Duration {
    Duration::from_secs(config.request_timeout_seconds.max(MINIMUM_TIMEOUT_SECONDS))
}

pub(crate) fn authorize(config: &Config, request: RequestBuilder) -> RequestBuilder {
    match &config.api_token {
        Some(token) => request.header(config.token_header.as_str(), token),
        None => request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    #[tokio::test]
    async fn a_redirect_to_another_host_is_returned_rather_than_followed() {
        let origin = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/history/prompt"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/collect", elsewhere.uri())),
            )
            .mount(&origin)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&elsewhere)
            .await;
        let config = Config {
            api_token: Some("secret".into()),
            ..Config::default()
        };

        let response = authorize(
            &config,
            client(&config)
                .unwrap()
                .get(format!("{}/history/prompt", origin.uri())),
        )
        .send()
        .await
        .unwrap();

        assert_eq!(response.status(), 307);
        assert!(
            elsewhere.received_requests().await.unwrap().is_empty(),
            "the token must not follow a redirect to another host"
        );
    }

    #[test]
    fn a_request_timeout_of_zero_is_raised_to_the_floor() {
        let config = Config {
            request_timeout_seconds: 0,
            ..Config::default()
        };
        assert_eq!(
            request_timeout(&config),
            Duration::from_secs(MINIMUM_TIMEOUT_SECONDS)
        );
    }
}
