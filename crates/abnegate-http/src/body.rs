use crate::error::HttpError;
use crate::error::Result;

/// Read at most `limit` bytes of `response`, refusing a body that does not fit
/// rather than buffering it whole and measuring it afterwards.
pub async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let declared = response.content_length();
    if declared.is_some_and(|length| u64::try_from(limit).is_ok_and(|limit| length > limit)) {
        return Err(HttpError::OversizedBody { limit });
    }

    let mut body = Vec::with_capacity(
        declared
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default(),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| HttpError::UnreadableBody(error.without_url()))?
    {
        if chunk.len() > limit - body.len() {
            return Err(HttpError::OversizedBody { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::serve_once;

    async fn body_of(length: usize) -> reqwest::Response {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("b".repeat(length)))
            .mount(&server)
            .await;

        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a client builds")
            .get(server.uri())
            .send()
            .await
            .expect("the mock server answers")
    }

    async fn raw(response: &'static [u8], query: &str) -> reqwest::Response {
        let port = serve_once(response).await;

        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a client builds")
            .get(format!("http://127.0.0.1:{port}/{query}"))
            .send()
            .await
            .expect("the server answers with headers")
    }

    #[tokio::test]
    async fn a_body_inside_the_cap_is_read_whole() {
        let body = read_capped(body_of(64).await, 128)
            .await
            .expect("64 bytes fit under 128");

        assert_eq!(body.len(), 64);
    }

    #[tokio::test]
    async fn a_body_the_size_of_the_cap_is_still_read() {
        let body = read_capped(body_of(128).await, 128)
            .await
            .expect("the cap is inclusive");

        assert_eq!(body.len(), 128);
    }

    #[tokio::test]
    async fn a_body_declared_over_the_cap_is_refused() {
        let error = read_capped(body_of(512).await, 128)
            .await
            .expect_err("512 bytes do not fit under 128");

        assert!(
            matches!(error, HttpError::OversizedBody { limit: 128 }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_chunked_body_is_refused_once_it_outgrows_the_cap() {
        let response = raw(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n40\r\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n40\r\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n0\r\n\r\n",
            "",
        )
        .await;

        let error = read_capped(response, 100)
            .await
            .expect_err("128 streamed bytes do not fit under 100");

        assert!(
            matches!(error, HttpError::OversizedBody { limit: 100 }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_body_cut_short_is_unreadable_and_does_not_repeat_the_url() {
        let response = raw(
            b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nshort",
            "?key=hunter2",
        )
        .await;

        let error = read_capped(response, 1_024)
            .await
            .expect_err("the body ended early");

        assert!(matches!(error, HttpError::UnreadableBody(_)), "{error}");
        assert!(!format!("{error:?}").contains("hunter2"), "{error:?}");
    }
}
