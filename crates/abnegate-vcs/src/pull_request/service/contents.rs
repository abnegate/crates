use super::*;
use crate::commit_sha::CommitSha;
use crate::parse_error::ParseError;
use crate::pull_request::Contents;
use crate::pull_request::Excerpt;
use crate::pull_request::github_contents_answer::GitHubContentsAnswer;
use crate::repository_path::RepositoryPath;
use crate::tag_name::TagName;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::str;

/// The media type GitHub answers with a pull request's unified diff in.
const DIFF: &str = "application/vnd.github.diff";

/// The media type GitHub describes a directory's entries in, whatever was
/// asked for. A file asked for raw never comes back in it, even one that holds
/// JSON.
const LISTING: &str = "application/json";

/// What separates a media type from the parameters after it.
const PARAMETERS: char = ';';

/// What an answer describing a directory, or another entry that is not a
/// file, is reported as.
const NOT_A_FILE: &str = "The path names a directory or another entry that is not a file";

/// The query parameter that names the ref a file is read at.
const REFERENCE: &str = "ref";

/// What GitHub breaks a file's base64 content into lines with.
const LINE_BREAK: char = '\n';

impl PullRequestService {
    /// The start of a pull request's unified diff, read no further than
    /// `limit` bytes.
    ///
    /// The excerpt is never longer than `limit` bytes, never ends inside a
    /// character, and says whether the diff went on past what it holds.
    pub async fn fetch_diff(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        limit: usize,
    ) -> PullRequestResult<Excerpt> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.repository_endpoint(repository, &["pulls", &number])?;

        let response = self
            .answered(self.request(Method::GET, url, token, DIFF)?)
            .await?;
        excerpt(response, limit).await
    }

    /// The start of one file as it is at `revision`, read no further than
    /// `limit` bytes.
    ///
    /// Each segment of `path` travels as one percent-encoded path segment, so
    /// a path someone else chose reaches this repository's contents and
    /// nothing beside them. A path that names a directory, or anything else
    /// that is not a file, is refused without its description being read.
    ///
    /// The excerpt is never longer than `limit` bytes, never ends inside a
    /// character, and says whether the file went on past what it holds.
    pub async fn fetch_file(
        &self,
        repository: &Repository,
        token: &SecretValue,
        path: &RepositoryPath,
        revision: &CommitSha,
        limit: usize,
    ) -> PullRequestResult<Excerpt> {
        let segments: Vec<&str> = ["contents"].into_iter().chain(path.segments()).collect();
        let mut url = self.repository_endpoint(repository, &segments)?;
        url.query_pairs_mut()
            .append_pair(REFERENCE, revision.as_str());

        let response = self
            .answered(self.request(Method::GET, url, token, RAW)?)
            .await?;
        if listing(response.headers()) {
            return Err(PullRequestError::GitHubApi(NOT_A_FILE.to_string()));
        }
        excerpt(response, limit).await
    }

    /// One file as it is at `reference`, a branch, a tag, a commit, or `HEAD`
    /// for the tip of the default branch, as GitHub's contents API describes
    /// it, its base64 content decoded to bytes.
    ///
    /// `reference` is held to the rules [`Self::fetch_tree_at`] holds it to,
    /// and travels as the `ref` query value. Each segment of `path` travels
    /// as one percent-encoded path segment, as for [`Self::fetch_file`]. The
    /// answer is read no further than any other JSON answer is.
    ///
    /// A path that names a directory, which GitHub answers with a list of its
    /// entries, is [`PullRequestError::GitHubApi`], and so is an answer this
    /// crate cannot read, which is never quoted. Content that is not valid
    /// base64 is [`PullRequestError::MalformedContent`]. Content GitHub left
    /// out is `None`, and so is a size it left out. GitHub sends a file past a
    /// megabyte as empty content, which reads as no bytes beside the size
    /// GitHub reports, so [`Contents::size`] tells it from an empty file.
    pub async fn fetch_contents(
        &self,
        repository: &Repository,
        token: &SecretValue,
        path: &RepositoryPath,
        reference: &str,
    ) -> PullRequestResult<Contents> {
        if !TagName::accepts_reference(reference) {
            return Err(ParseError::BranchName(reference.to_string()).into());
        }
        let segments: Vec<&str> = ["contents"].into_iter().chain(path.segments()).collect();
        let mut url = self.repository_endpoint(repository, &segments)?;
        url.query_pairs_mut().append_pair(REFERENCE, reference);

        let described = match self.get(url, token).await? {
            GitHubContentsAnswer::Listing(_) => {
                return Err(PullRequestError::GitHubApi(NOT_A_FILE.to_string()));
            }
            GitHubContentsAnswer::File(described) => described,
        };
        let content = described.content.map(decoded).transpose()?;
        Ok(Contents::new(content, described.size))
    }
}

/// The bytes `encoded` holds, once the line breaks GitHub wraps base64 in are
/// dropped from it in place.
fn decoded(mut encoded: String) -> PullRequestResult<Vec<u8>> {
    encoded.retain(|character| character != LINE_BREAK);
    STANDARD
        .decode(encoded)
        .map_err(|_| PullRequestError::MalformedContent)
}

/// Whether an answer's `Content-Type` says it describes a directory's entries
/// rather than holding a file's bytes.
fn listing(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(PARAMETERS).next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case(LISTING))
}

/// The first `limit` bytes of an answer's body, read no further, as text.
async fn excerpt(response: Response, limit: usize) -> PullRequestResult<Excerpt> {
    let (prefix, truncated) = read_prefix(response, limit).await?;
    Ok(cut(prefix, truncated, limit))
}

/// `prefix` as text of at most `limit` bytes.
///
/// A prefix the limit cut short loses the start of a character the limit split.
/// Bytes that are not text read as replacement characters, each three bytes
/// long, so text that grows past `limit` that way is cut again, on a character
/// boundary, and counts as truncated.
fn cut(mut prefix: Vec<u8>, mut truncated: bool, limit: usize) -> Excerpt {
    if truncated {
        prefix.truncate(complete(&prefix));
    }
    let mut text = match String::from_utf8(prefix) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    };
    if text.len() > limit {
        text.truncate(text.floor_char_boundary(limit));
        truncated = true;
    }
    Excerpt { text, truncated }
}

/// How many of `bytes` remain once a character they end partway through is
/// dropped. A byte that can never be text is kept, wherever it is.
fn complete(bytes: &[u8]) -> usize {
    let mut start = 0;
    loop {
        match str::from_utf8(&bytes[start..]) {
            Ok(_) => return bytes.len(),
            Err(error) => match error.error_len() {
                Some(invalid) => start += error.valid_up_to() + invalid,
                None => return start + error.valid_up_to(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_id::ObjectId;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use reqwest::header::AUTHORIZATION;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    const PULL_REQUEST: &str = "/repos/acme/project/pulls/7";

    async fn serving(body: &[u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULL_REQUEST))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        server
    }

    fn repository_path(value: &str) -> RepositoryPath {
        RepositoryPath::parse(value).unwrap()
    }

    #[tokio::test]
    async fn the_diff_is_cut_at_the_budget() {
        let body = "diff --git a/x b/x\n+".repeat(20);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULL_REQUEST))
            .and(header("accept", DIFF))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let diff = service
            .fetch_diff(&seven(&service), &token(), 50)
            .await
            .unwrap();

        assert!(diff.truncated);
        assert!(diff.text.len() <= 50, "{}", diff.text.len());
        assert_eq!(diff.text, body[..50]);
        assert!(diff.to_string().ends_with(Excerpt::MARKER), "{diff}");
    }

    #[tokio::test]
    async fn a_diff_within_the_budget_is_whole() {
        let body = "diff --git a/x b/x\n+fn main() {}\n";
        let server = serving(body.as_bytes()).await;
        let service = stand_in(&server).await;

        for limit in [body.len() * 2, body.len()] {
            let diff = service
                .fetch_diff(&seven(&service), &token(), limit)
                .await
                .unwrap();

            assert_eq!(
                diff,
                Excerpt {
                    text: body.to_string(),
                    truncated: false,
                },
                "limit {limit}"
            );
            assert_eq!(diff.to_string(), body, "limit {limit}");
        }

        let server = serving(b"").await;
        let service = stand_in(&server).await;
        let empty = service
            .fetch_diff(&seven(&service), &token(), 0)
            .await
            .unwrap();
        assert_eq!(
            empty,
            Excerpt {
                text: String::new(),
                truncated: false,
            }
        );
    }

    #[tokio::test]
    async fn a_cut_never_splits_a_character() {
        let body = "a\u{e9}\u{20ac}\u{1f980}b";
        let server = serving(body.as_bytes()).await;
        let service = stand_in(&server).await;

        for limit in 0..=body.len() {
            let diff = service
                .fetch_diff(&seven(&service), &token(), limit)
                .await
                .unwrap();

            assert_eq!(
                diff.text,
                body[..body.floor_char_boundary(limit)],
                "limit {limit}"
            );
            assert_eq!(diff.truncated, limit < body.len(), "limit {limit}");
        }

        let malformed = serving(&[b'a', 0xFF, b'b', 0xE2, 0x82, 0xAC, b'c']).await;
        let service = stand_in(&malformed).await;
        let diff = service
            .fetch_diff(&seven(&service), &token(), 5)
            .await
            .unwrap();
        assert_eq!(
            diff,
            Excerpt {
                text: "a\u{fffd}b".to_string(),
                truncated: true,
            },
            "a byte that is not text before the cut must not stop a split character being dropped"
        );
    }

    #[test]
    fn only_a_character_cut_off_at_the_end_is_dropped() {
        for (bytes, kept) in [
            (&b""[..], 0),
            (b"abc", 3),
            (&[b'a', 0xE2, 0x82], 1),
            (&[0xF0, 0x9F, 0x98], 0),
            (&[b'a', 0xFF], 2),
            (&[0xE2, 0xFF], 2),
            (&[b'a', 0xFF, b'b', 0xE2, 0x82], 3),
            (&[0xE2, 0x82, 0xE2, 0x82], 2),
        ] {
            assert_eq!(complete(bytes), kept, "{bytes:x?}");
        }
    }

    #[tokio::test]
    async fn a_body_that_is_not_text_is_never_longer_than_its_limit() {
        let limit = 10;
        for length in [limit, limit * 4] {
            let server = serving(&vec![0xFF; length]).await;
            let service = stand_in(&server).await;

            let diff = service
                .fetch_diff(&seven(&service), &token(), limit)
                .await
                .unwrap();

            assert!(diff.text.len() <= limit, "{length} bytes: {:?}", diff.text);
            assert_eq!(diff.text, "\u{fffd}".repeat(3), "{length} bytes");
            assert!(diff.truncated, "{length} bytes");
        }
    }

    #[tokio::test]
    async fn a_file_is_read_raw_at_the_commit_under_review() {
        let revision = commit('a');
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/contents/src/cart.ts"))
            .and(query_param("ref", revision.as_str()))
            .and(header("accept", RAW))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("export const cart = [];\n"))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let file = service
            .fetch_file(
                &project(&service),
                &token(),
                &repository_path("src/cart.ts"),
                &revision,
                1_000,
            )
            .await
            .unwrap();

        assert_eq!(
            file,
            Excerpt {
                text: "export const cart = [];\n".to_string(),
                truncated: false,
            }
        );
        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received[0].url.query(),
            Some(format!("ref={}", revision.as_str()).as_str())
        );
    }

    #[tokio::test]
    async fn a_file_path_with_spaces_and_hashes_is_sent_as_encoded_segments() {
        for (value, encoded) in [
            (
                "docs/my file.md",
                "/repos/acme/project/contents/docs/my%20file.md",
            ),
            (
                "notes/#1 plan?.md",
                "/repos/acme/project/contents/notes/%231%20plan%3F.md",
            ),
            ("a%2Fb/c.rs", "/repos/acme/project/contents/a%252Fb/c.rs"),
            (
                "docs/caf\u{e9}.md",
                "/repos/acme/project/contents/docs/caf%C3%A9.md",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(encoded))
                .and(query_param("ref", commit('b').as_str()))
                .respond_with(ResponseTemplate::new(200).set_body_string("kept"))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let file = service
                .fetch_file(
                    &project(&service),
                    &token(),
                    &repository_path(value),
                    &commit('b'),
                    100,
                )
                .await
                .unwrap_or_else(|error| panic!("{value:?} was not sent as {encoded}: {error}"));

            assert_eq!(file.text, "kept", "{value:?}");
        }
    }

    #[tokio::test]
    async fn a_path_naming_a_directory_is_not_read_as_a_file() {
        for content_type in [
            "application/json; charset=utf-8",
            "application/json",
            "Application/JSON ; charset=utf-8",
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/contents/src"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    r#"[{"name":"LISTING-SENTINEL","type":"file"}]"#,
                    content_type,
                ))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_file(
                    &project(&service),
                    &token(),
                    &repository_path("src"),
                    &commit('a'),
                    1_000,
                )
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref message) if message == NOT_A_FILE),
                "{content_type}: {failure:?}"
            );
            assert!(
                !format!("{failure} {failure:?}").contains("SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_json_file_returned_raw_is_read_rather_than_taken_for_a_directory() {
        let manifest = r#"{"name":"shop","private":true}"#;
        for content_type in [RAW, "application/vnd.github.raw+json; charset=utf-8"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/contents/package.json"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(manifest, content_type))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let file = service
                .fetch_file(
                    &project(&service),
                    &token(),
                    &repository_path("package.json"),
                    &commit('a'),
                    1_000,
                )
                .await
                .unwrap_or_else(|error| panic!("{content_type}: {error:?}"));

            assert_eq!(file.text, manifest, "{content_type}");
            assert!(!file.truncated, "{content_type}");
        }
    }

    #[tokio::test]
    async fn a_diff_or_file_github_refuses_is_reported_as_what_it_is() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULL_REQUEST))
            .respond_with(ResponseTemplate::new(406).set_body_json(serde_json::json!({
                "message": "Sorry, the diff exceeded the maximum number of files (300).",
                "documentation_url": "RAW-SENTINEL",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/contents/missing.rs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/contents/limited.rs"))
            .respond_with(ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_diff(&seven(&service), &token(), 100)
            .await
            .unwrap_err();
        assert!(
            matches!(
                failure,
                PullRequestError::Refused { status: 406, ref message }
                    if message == "Sorry, the diff exceeded the maximum number of files (300)."
            ),
            "{failure:?}"
        );
        assert!(!format!("{failure:?}").contains("SENTINEL"), "{failure:?}");

        for (value, expected) in [
            ("missing.rs", PullRequestError::NotFound),
            ("limited.rs", PullRequestError::RateLimited),
        ] {
            let failure = service
                .fetch_file(
                    &project(&service),
                    &token(),
                    &repository_path(value),
                    &commit('a'),
                    100,
                )
                .await
                .unwrap_err();
            assert_eq!(
                std::mem::discriminant(&failure),
                std::mem::discriminant(&expected),
                "{value}: {failure:?}"
            );
        }

        let rejected = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&rejected)
            .await;
        let service = stand_in(&rejected).await;
        let failure = service
            .fetch_diff(&seven(&service), &token(), 100)
            .await
            .unwrap_err();
        assert!(
            matches!(failure, PullRequestError::AuthenticationFailed),
            "{failure:?}"
        );
    }

    const README: &str = "/repos/acme/project/contents/README.md";

    /// `content` in base64 broken into lines of sixty characters, each ended
    /// by a line break, as GitHub's contents API sends a file.
    fn wrapped(content: &[u8]) -> String {
        STANDARD
            .encode(content)
            .as_bytes()
            .chunks(60)
            .map(|line| format!("{}\n", str::from_utf8(line).unwrap()))
            .collect()
    }

    fn described(name: &str, content: Option<&str>) -> Value {
        let mut answer = json!({
            "name": name,
            "path": name,
            "sha": commit('b').as_str(),
            "size": 100,
            "type": "file",
            "encoding": "base64",
        });
        if let Some(content) = content {
            answer["content"] = json!(content);
        }
        answer
    }

    async fn describing(route: &str, reference: &str, body: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route))
            .and(query_is(&[("ref", reference)]))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn contents_of(
        server: &MockServer,
        value: &str,
        reference: &str,
    ) -> PullRequestResult<Contents> {
        let service = stand_in(server).await;
        service
            .fetch_contents(
                &project(&service),
                &token(),
                &repository_path(value),
                reference,
            )
            .await
    }

    #[tokio::test]
    async fn a_file_s_base64_broken_into_lines_decodes_to_its_exact_bytes() {
        let text = "# Notes\n\n".to_string() + &"Every line of this file is kept.\n".repeat(8);
        let encoded = wrapped(text.as_bytes());
        assert!(encoded.matches('\n').count() > 1, "{encoded:?}");
        let server = describing(README, "main", described("README.md", Some(&encoded))).await;

        let contents = contents_of(&server, "README.md", "main").await.unwrap();

        assert_eq!(contents, Contents::new(Some(text.into_bytes()), Some(100)));
    }

    #[tokio::test]
    async fn empty_content_is_a_file_of_no_bytes() {
        let server = describing(README, "main", described("README.md", Some(""))).await;

        let contents = contents_of(&server, "README.md", "main").await.unwrap();

        assert_eq!(contents.content, Some(Vec::new()));
    }

    #[tokio::test]
    async fn a_file_github_declined_to_inline_reads_as_no_bytes_of_the_size_it_reported() {
        for (size, content) in [(2_000_000, ""), (0, "")] {
            let server = describing(
                README,
                "main",
                json!({
                    "name": "README.md",
                    "path": "README.md",
                    "sha": commit('b').as_str(),
                    "size": size,
                    "type": "file",
                    "content": content,
                    "encoding": if size == 0 { "base64" } else { "none" },
                }),
            )
            .await;

            let contents = contents_of(&server, "README.md", "main").await.unwrap();

            assert_eq!(
                contents,
                Contents::new(Some(Vec::new()), Some(size)),
                "size {size}"
            );
        }
    }

    #[tokio::test]
    async fn an_answer_without_content_holds_none() {
        let server = describing(README, "main", described("README.md", None)).await;

        let contents = contents_of(&server, "README.md", "main").await.unwrap();

        assert_eq!(contents.content, None);
    }

    #[tokio::test]
    async fn content_that_is_not_base64_is_malformed_and_never_repeated() {
        let server = describing(
            README,
            "main",
            described("README.md", Some("BASE64-SENTINEL!*")),
        )
        .await;

        let failure = contents_of(&server, "README.md", "main").await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::MalformedContent),
            "{failure:?}"
        );
        assert_eq!(
            failure.to_string(),
            "GitHub sent file content that is not valid base64"
        );
        assert!(
            !format!("{failure} {failure:?}").contains("SENTINEL"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_directory_s_listing_is_not_read_as_a_file() {
        for listing in [
            json!([{ "name": "LISTING-SENTINEL", "type": "file" }]),
            json!(["LISTING-SENTINEL", 7]),
            json!([]),
        ] {
            let server =
                describing("/repos/acme/project/contents/src", "main", listing.clone()).await;

            let failure = contents_of(&server, "src", "main").await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref message) if message == NOT_A_FILE),
                "{listing}: {failure:?}"
            );
            assert!(
                !format!("{failure} {failure:?}").contains("SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_answer_that_describes_no_file_is_unreadable_and_never_quoted() {
        for answer in [
            json!({ "content": 7, "name": "ANSWER-SENTINEL" }),
            json!({ "content": "", "size": "ANSWER-SENTINEL" }),
            json!({ "content": "", "size": -1, "name": "ANSWER-SENTINEL" }),
            json!("ANSWER-SENTINEL"),
        ] {
            let server = describing(README, "main", answer.clone()).await;

            let failure = contents_of(&server, "README.md", "main").await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref message) if message == UNREADABLE),
                "{answer}: {failure:?}"
            );
            assert!(
                !format!("{failure} {failure:?}").contains("SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_file_is_described_by_its_segments_at_the_ref_asked_for() {
        let server = describing(
            "/repos/acme/project/contents/src/lib.rs",
            "main",
            described("lib.rs", Some(&wrapped(b"pub fn run() {}\n"))),
        )
        .await;

        let contents = contents_of(&server, "src/lib.rs", "main").await.unwrap();

        assert_eq!(contents.content.as_deref(), Some(&b"pub fn run() {}\n"[..]));
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.query(), Some("ref=main"));
        assert_eq!(
            received[0]
                .headers
                .get("accept")
                .and_then(|value| value.to_str().ok()),
            Some("application/vnd.github+json")
        );
        assert_eq!(
            received[0]
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer token")
        );
    }

    #[tokio::test]
    async fn a_segment_with_a_space_and_a_ref_with_a_query_s_marks_travel_encoded() {
        let reference = "fix/bug-123&feature=new";
        let server = describing(
            "/repos/acme/project/contents/docs/my%20file.md",
            reference,
            described("my file.md", Some("")),
        )
        .await;

        let contents = contents_of(&server, "docs/my file.md", reference)
            .await
            .unwrap();

        assert_eq!(contents.content, Some(Vec::new()));
        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received[0].url.query(),
            Some("ref=fix%2Fbug-123%26feature%3Dnew")
        );
    }

    #[tokio::test]
    async fn a_ref_git_would_refuse_is_refused_before_a_file_is_described() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        for reference in ["", "a..b", "@"] {
            let failure = contents_of(&server, "README.md", reference)
                .await
                .unwrap_err();

            assert!(
                matches!(
                    &failure,
                    PullRequestError::Parse(ParseError::BranchName(value)) if value == reference
                ),
                "{reference:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_anonymous_file_read_carries_no_authorization() {
        let server = describing(README, "HEAD", described("README.md", Some(""))).await;
        let service = stand_in(&server).await.with_anonymous_reads();

        let contents = service
            .fetch_contents(
                &project(&service),
                &SecretValue::new(""),
                &repository_path("README.md"),
                "HEAD",
            )
            .await
            .unwrap();

        assert_eq!(contents.content, Some(Vec::new()));
        let received = server.received_requests().await.unwrap();
        assert!(
            !received[0].headers.contains_key(AUTHORIZATION),
            "an anonymous read carries no authorization"
        );
    }

    #[tokio::test]
    async fn a_file_github_holds_nothing_for_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/contents/missing.rs"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let failure = contents_of(&server, "missing.rs", "main")
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn an_answer_with_nothing_but_its_content_is_still_read() {
        let server = describing(README, "main", json!({ "content": wrapped(b"bare") })).await;

        let contents = contents_of(&server, "README.md", "main").await.unwrap();

        assert_eq!(contents, Contents::new(Some(b"bare".to_vec()), None));
    }

    #[tokio::test]
    async fn a_repository_is_read_from_its_default_branch_down_to_each_file_s_bytes() {
        let readme = "# Test Repo\n\nThis is a test repository.";
        let library = "pub fn hello() -> &'static str {\n    \"Hello, world!\"\n}";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "project",
                "default_branch": "main",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/git/trees/main"))
            .and(query_is(&[("recursive", "1")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": commit('a').as_str(),
                "tree": [
                    {
                        "path": "README.md",
                        "mode": "100644",
                        "type": "blob",
                        "size": 100,
                        "sha": commit('b').as_str(),
                    },
                    {
                        "path": "src/lib.rs",
                        "mode": "100644",
                        "type": "blob",
                        "size": 500,
                        "sha": commit('c').as_str(),
                    },
                    {
                        "path": "image.png",
                        "mode": "100644",
                        "type": "blob",
                        "size": 1000,
                        "sha": commit('d').as_str(),
                    },
                ],
                "truncated": false,
            })))
            .expect(1)
            .mount(&server)
            .await;
        for (route, name, text) in [
            (README, "README.md", readme),
            ("/repos/acme/project/contents/src/lib.rs", "lib.rs", library),
        ] {
            Mock::given(method("GET"))
                .and(path(route))
                .and(query_is(&[("ref", "main")]))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(described(name, Some(&STANDARD.encode(text)))),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let service = stand_in(&server).await;
        let repository = project(&service);

        let branch = service
            .get_default_branch(&repository, &token())
            .await
            .unwrap();
        let tree = service
            .fetch_tree_at(&repository, &token(), branch.as_str(), true)
            .await
            .unwrap();
        let mut read = Vec::new();
        for entry in tree
            .entries
            .iter()
            .filter(|entry| entry.path != "image.png")
        {
            let contents = service
                .fetch_contents(
                    &repository,
                    &token(),
                    &repository_path(&entry.path),
                    branch.as_str(),
                )
                .await
                .unwrap();
            read.push((entry.path.clone(), contents.content));
        }

        assert_eq!(tree.sha, ObjectId::from(commit('a')));
        assert_eq!(
            read,
            [
                ("README.md".to_string(), Some(readme.as_bytes().to_vec())),
                ("src/lib.rs".to_string(), Some(library.as_bytes().to_vec())),
            ]
        );
    }
}
