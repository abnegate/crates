use super::*;
use crate::blob::Blob;
use crate::object_id::ObjectId;

impl GitLabService {
    /// A blob's bytes, whole and exactly as git stores them, so long as there
    /// are no more than `limit` of them.
    ///
    /// GitLab's answer is read no further than `limit` bytes, the caller's
    /// bound on what it holds in memory; [`usize::MAX`] reads a blob of any
    /// size. An answer longer than that is [`GitLabError::TooLarge`] and is
    /// never returned in part; one whose declared length is longer is refused
    /// before any of it is read.
    pub async fn fetch_blob(
        &self,
        project: &Project,
        token: &SecretValue,
        blob: &ObjectId,
        limit: usize,
    ) -> GitLabResult<Blob> {
        let url = self.project_endpoint(project, &["repository", "blobs", blob.as_str(), "raw"]);
        let response = self.answered(self.request(url, token)?).await?;
        let content = read(response, limit, || GitLabError::TooLarge).await?;
        Ok(Blob::new(blob.clone(), content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab::service::fixtures::API;
    use crate::gitlab::service::fixtures::empty;
    use crate::gitlab::service::fixtures::object;
    use crate::gitlab::service::fixtures::project;
    use crate::gitlab::service::fixtures::served;
    use crate::gitlab::service::fixtures::token;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// An answer that declares eleven bytes and then hangs up before sending
    /// any of them.
    const DECLARED_ELEVEN: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\n";

    fn route() -> String {
        format!(
            "{API}/projects/acme%2Fproject/repository/blobs/{}/raw",
            object('f')
        )
    }

    async fn serving(content: &[u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .and(header("private-token", "token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.to_vec()))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn blob_of(server: &MockServer, limit: usize) -> GitLabResult<Blob> {
        served(server)
            .fetch_blob(&project(), &token(), &object('f'), limit)
            .await
    }

    /// A server on a loopback port that answers every request with
    /// `answer`, whatever it asks, and then hangs up.
    async fn hanging_up(answer: &'static [u8]) -> GitLabService {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port to listen on");
        let address = listener
            .local_addr()
            .expect("the address the listener is bound to");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap_or_default();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let _ = stream.write_all(answer).await;
            }
        });
        GitLabService::configured_allowing_http(&format!("http://{address}")).unwrap()
    }

    #[tokio::test]
    async fn a_blob_is_returned_byte_for_byte() {
        let content: Vec<u8> = (0..=u8::MAX).rev().cycle().take(3_001).collect();
        assert_eq!(content[0], 0xff);
        assert!(std::str::from_utf8(&content).is_err(), "not text");
        let server = serving(&content).await;

        let blob = blob_of(&server, content.len()).await.unwrap();

        assert_eq!(blob, Blob::new(object('f'), content));
    }

    #[tokio::test]
    async fn an_empty_blob_is_read() {
        let server = serving(b"").await;

        let blob = blob_of(&server, 0).await.unwrap();

        assert_eq!(blob, Blob::new(object('f'), Vec::new()));
    }

    #[tokio::test]
    async fn a_blob_exactly_at_the_limit_is_read() {
        let content = b"twelve bytes";
        let server = serving(content).await;

        let blob = blob_of(&server, content.len()).await.unwrap();

        assert_eq!(blob.content, content);
    }

    #[tokio::test]
    async fn a_blob_one_byte_over_the_limit_is_too_large() {
        let content = b"twelve bytes";
        let server = serving(content).await;

        let failure = blob_of(&server, content.len() - 1).await.unwrap_err();

        assert!(matches!(failure, GitLabError::TooLarge), "{failure:?}");
    }

    #[tokio::test]
    async fn a_blob_declared_past_the_limit_is_too_large_before_it_is_read() {
        let service = hanging_up(DECLARED_ELEVEN).await;

        let failure = service
            .fetch_blob(&project(), &token(), &object('f'), 10)
            .await
            .unwrap_err();

        assert!(matches!(failure, GitLabError::TooLarge), "{failure:?}");
    }

    #[tokio::test]
    async fn a_blob_cut_short_is_an_http_error() {
        let service = hanging_up(DECLARED_ELEVEN).await;

        let failure = service
            .fetch_blob(&project(), &token(), &object('f'), 11)
            .await
            .unwrap_err();

        assert!(matches!(failure, GitLabError::Http(_)), "{failure:?}");
    }

    #[tokio::test]
    async fn no_limit_reads_the_whole_blob() {
        let content = vec![b'x'; 1024 * 1024];
        let server = serving(&content).await;

        let blob = blob_of(&server, usize::MAX).await.unwrap();

        assert_eq!(blob.content, content);
    }

    #[tokio::test]
    async fn an_anonymous_blob_read_sends_no_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .expect(1)
            .mount(&server)
            .await;

        let blob = served(&server)
            .with_anonymous_reads()
            .fetch_blob(&project(), &empty(), &object('f'), usize::MAX)
            .await
            .unwrap();

        assert_eq!(blob.content, b"hello");
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("private-token"));
    }

    #[tokio::test]
    async fn a_missing_blob_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let failure = blob_of(&server, usize::MAX).await.unwrap_err();

        assert!(matches!(failure, GitLabError::NotFound), "{failure:?}");
    }
}
