use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::Blob;
use crate::pull_request::Tree;
use crate::pull_request::TreeEntry;
use crate::pull_request::github_blob::GitHubBlob;
use crate::pull_request::github_tree::GitHubTree;
use crate::pull_request::github_tree_entry::GitHubTreeEntry;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The query parameter that asks GitHub for every entry beneath a tree, and
/// the value that turns it on.
const RECURSIVE: (&str, &str) = ("recursive", "1");

/// The only encoding of a blob's content this crate reads.
const BASE64: &str = "base64";

/// What GitHub breaks a blob's base64 into lines with.
const LINE_BREAK: char = '\n';

/// Bytes of content each four characters of base64 hold.
const DECODED_GROUP: usize = 3;

/// Characters of base64 that hold one group of decoded bytes.
const ENCODED_GROUP: usize = 4;

/// The longest line of base64 GitHub writes a blob's content in.
const LINE_LENGTH: usize = 60;

/// Bytes a line break takes in a JSON string, written as the escape `\n`.
const ESCAPED_LINE_BREAK: usize = 2;

/// Bytes allowed for the rest of a blob's answer: its object name, node
/// identifier, URL, size and encoding.
const ENVELOPE: usize = 4 * 1024;

impl PullRequestService {
    /// A git tree's entries, read without following the trees within it
    /// unless `recursive` asks to.
    ///
    /// Read without recursing, each entry's path is its name within the tree,
    /// so a caller can walk a path one tree at a time and check every entry
    /// it passes on the way, as one refusing symbolic links and submodules
    /// must. [`Tree::truncated`] says whether GitHub cut the list short; a
    /// caller that needs every entry must not take a truncated tree for a
    /// whole one.
    ///
    /// An entry whose object name is not a full sha is
    /// [`PullRequestError::Parse`]; no part of the tree is returned.
    pub async fn fetch_tree(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tree: &CommitSha,
        recursive: bool,
    ) -> PullRequestResult<Tree> {
        let mut url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "git",
            "trees",
            tree.as_str(),
        ]);
        if recursive {
            let (name, value) = RECURSIVE;
            url.query_pairs_mut().append_pair(name, value);
        }

        let answer: GitHubTree = self.get(url, token).await?;
        let entries = answer
            .tree
            .into_iter()
            .map(entry)
            .collect::<PullRequestResult<Vec<TreeEntry>>>()?;
        Ok(Tree::new(CommitSha::parse(&answer.sha)?, entries).with_truncated(answer.truncated))
    }

    /// A git blob's content, whole, so long as it is no larger than `limit`
    /// bytes.
    ///
    /// The answer is read no further than the base64 that `limit` bytes take,
    /// with its line breaks and the rest of the answer, and never past
    /// 16 MiB. A blob GitHub says is larger than `limit`, or an answer longer
    /// than that, is [`PullRequestError::TooLarge`] and is not decoded.
    ///
    /// Content in an encoding other than base64, base64 that does not decode,
    /// and content whose decoded length is not the size GitHub gave are each
    /// [`PullRequestError::GitHubApi`].
    pub async fn fetch_blob(
        &self,
        repository: &Repository,
        token: &SecretValue,
        blob: &CommitSha,
        limit: usize,
    ) -> PullRequestResult<Blob> {
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "git",
            "blobs",
            blob.as_str(),
        ]);

        let response = answered(self.request(Method::GET, url, token, ACCEPT)).await?;
        let (body, more) = read_prefix(response, encoded_bound(limit)).await?;
        if more {
            return Err(PullRequestError::TooLarge);
        }
        let answer: GitHubBlob = serde_json::from_slice(&body).map_err(|_| unreadable())?;
        if !usize::try_from(answer.size).is_ok_and(|size| size <= limit) {
            return Err(PullRequestError::TooLarge);
        }
        let content = decoded(&answer)?;
        Ok(Blob::new(CommitSha::parse(&answer.sha)?, content))
    }
}

/// A tree entry GitHub listed, its object name parsed.
fn entry(listed: GitHubTreeEntry) -> PullRequestResult<TreeEntry> {
    let entry = TreeEntry::new(
        listed.path,
        listed.mode,
        listed.kind,
        CommitSha::parse(&listed.sha)?,
    );
    Ok(match listed.size {
        Some(size) => entry.with_size(size),
        None => entry,
    })
}

/// The most bytes of a blob's answer read for content of up to `limit` bytes:
/// the base64 those bytes take, a line break for every line of it, and the
/// rest of the answer, saturating and never past [`MAXIMUM_ANSWER_BYTES`].
fn encoded_bound(limit: usize) -> usize {
    let encoded = limit.div_ceil(DECODED_GROUP).saturating_mul(ENCODED_GROUP);
    let breaks = encoded
        .div_ceil(LINE_LENGTH)
        .saturating_mul(ESCAPED_LINE_BREAK);
    encoded
        .saturating_add(breaks)
        .saturating_add(ENVELOPE)
        .min(MAXIMUM_ANSWER_BYTES)
}

/// A blob's content decoded from base64 split across lines, refused unless
/// it decodes to exactly the size GitHub gave.
fn decoded(answer: &GitHubBlob) -> PullRequestResult<Vec<u8>> {
    if answer.encoding != BASE64 {
        return Err(unreadable());
    }
    let joined: String = answer
        .content
        .chars()
        .filter(|character| *character != LINE_BREAK)
        .collect();
    let content = STANDARD.decode(joined).map_err(|_| unreadable())?;
    match u64::try_from(content.len()).is_ok_and(|length| length == answer.size) {
        true => Ok(content),
        false => Err(unreadable()),
    }
}

/// What a blob answer this crate cannot read is reported as.
fn unreadable() -> PullRequestError {
    PullRequestError::GitHubApi(UNREADABLE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::TreeEntryKind;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn trees(sha: &CommitSha) -> String {
        format!("/repos/acme/project/git/trees/{sha}")
    }

    fn blobs(sha: &CommitSha) -> String {
        format!("/repos/acme/project/git/blobs/{sha}")
    }

    async fn serving(route: String, body: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("accept", ACCEPT))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    fn listing(truncated: bool) -> Value {
        json!({
            "sha": commit('a').as_str(),
            "url": "https://api.github.com/repos/acme/project/git/trees/a",
            "tree": [
                {
                    "path": "README.md",
                    "mode": "100644",
                    "type": "blob",
                    "sha": commit('b').as_str(),
                    "size": 30,
                },
                {
                    "path": "latest",
                    "mode": "120000",
                    "type": "blob",
                    "sha": commit('c').as_str(),
                    "size": 9,
                },
                {
                    "path": "src",
                    "mode": "040000",
                    "type": "tree",
                    "sha": commit('d').as_str(),
                },
                {
                    "path": "vendor",
                    "mode": "160000",
                    "type": "commit",
                    "sha": commit('e').as_str(),
                },
            ],
            "truncated": truncated,
        })
    }

    async fn tree_of(server: &MockServer, recursive: bool) -> PullRequestResult<Tree> {
        let service = stand_in(server).await;
        service
            .fetch_tree(&project(&service), &token(), &commit('a'), recursive)
            .await
    }

    /// GitHub's answer for a blob holding `content`: base64 in lines of 60,
    /// each ended by a line break, as GitHub writes it.
    fn encoded(content: &[u8]) -> String {
        STANDARD
            .encode(content)
            .as_bytes()
            .chunks(LINE_LENGTH)
            .map(|line| format!("{}\n", std::str::from_utf8(line).unwrap()))
            .collect()
    }

    fn blob_answer(size: usize, content: &str, encoding: &str) -> Value {
        json!({
            "sha": commit('f').as_str(),
            "node_id": "B_kwDOAAAAAdoAKGZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZm",
            "url": format!("https://api.github.com/repos/acme/project/git/blobs/{}", commit('f')),
            "size": size,
            "content": content,
            "encoding": encoding,
        })
    }

    async fn blob_of(server: &MockServer, limit: usize) -> PullRequestResult<Blob> {
        let service = stand_in(server).await;
        service
            .fetch_blob(&project(&service), &token(), &commit('f'), limit)
            .await
    }

    fn is_unreadable(failure: &PullRequestError) -> bool {
        matches!(failure, PullRequestError::GitHubApi(message) if message == UNREADABLE)
    }

    #[tokio::test]
    async fn a_tree_lists_each_entry_with_its_mode_and_kind() {
        let server = serving(trees(&commit('a')), listing(false)).await;

        let tree = tree_of(&server, false).await.unwrap();

        assert_eq!(
            tree,
            Tree::new(
                commit('a'),
                vec![
                    TreeEntry::new("README.md", "100644", TreeEntryKind::Blob, commit('b'))
                        .with_size(30),
                    TreeEntry::new("latest", "120000", TreeEntryKind::Blob, commit('c'))
                        .with_size(9),
                    TreeEntry::new("src", "040000", TreeEntryKind::Tree, commit('d')),
                    TreeEntry::new("vendor", "160000", TreeEntryKind::Commit, commit('e')),
                ],
            )
        );
        let regular: Vec<&str> = tree
            .entries
            .iter()
            .filter(|entry| entry.regular_file())
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(
            regular,
            ["README.md"],
            "a symbolic link, a directory and a submodule are not regular files"
        );
    }

    #[tokio::test]
    async fn a_tree_github_cut_short_says_so() {
        for truncated in [true, false] {
            let server = serving(trees(&commit('a')), listing(truncated)).await;

            let tree = tree_of(&server, false).await.unwrap();

            assert_eq!(tree.truncated, truncated);
            assert_eq!(tree.entries.len(), 4, "truncated {truncated}");
        }
    }

    #[tokio::test]
    async fn a_tree_is_read_recursively_only_when_asked() {
        for (recursive, query) in [(false, None), (true, Some("recursive=1"))] {
            let server = serving(trees(&commit('a')), listing(false)).await;

            tree_of(&server, recursive).await.unwrap();

            let received = server.received_requests().await.unwrap();
            assert_eq!(received[0].url.query(), query, "recursive {recursive}");
        }
    }

    #[tokio::test]
    async fn a_tree_that_does_not_say_whether_it_was_cut_short_is_unreadable() {
        let mut answer = listing(false);
        answer.as_object_mut().unwrap().remove("truncated");
        let server = serving(trees(&commit('a')), answer).await;

        let failure = tree_of(&server, false).await.unwrap_err();

        assert!(is_unreadable(&failure), "{failure:?}");
    }

    #[tokio::test]
    async fn a_tree_entry_with_a_malformed_sha_fails_the_whole_tree() {
        let mut answer = listing(false);
        answer["tree"][2]["sha"] = json!("not-a-sha");
        let server = serving(trees(&commit('a')), answer).await;

        let failure = tree_of(&server, false).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::CommitSha(_))),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_blob_is_decoded_from_the_lines_of_base64_github_writes() {
        let content: Vec<u8> = (0..=u8::MAX).cycle().take(3_001).collect();
        let body = encoded(&content);
        assert!(body.matches(LINE_BREAK).count() > 1, "{body}");
        let server = serving(
            blobs(&commit('f')),
            blob_answer(content.len(), &body, BASE64),
        )
        .await;

        let blob = blob_of(&server, content.len()).await.unwrap();

        assert_eq!(blob, Blob::new(commit('f'), content));
    }

    #[tokio::test]
    async fn an_empty_blob_is_decoded() {
        let server = serving(blobs(&commit('f')), blob_answer(0, "", BASE64)).await;

        let blob = blob_of(&server, 0).await.unwrap();

        assert_eq!(blob, Blob::new(commit('f'), Vec::new()));
    }

    #[tokio::test]
    async fn a_blob_github_says_is_larger_than_the_limit_is_too_large() {
        let content = b"twelve bytes";
        let server = serving(
            blobs(&commit('f')),
            blob_answer(content.len(), &encoded(content), BASE64),
        )
        .await;

        let failure = blob_of(&server, content.len() - 1).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::TooLarge), "{failure:?}");
    }

    #[tokio::test]
    async fn an_answer_longer_than_the_limit_takes_in_base64_is_too_large() {
        let limit = 10;
        let padding = "A".repeat(encoded_bound(limit) * 2);
        let server = serving(blobs(&commit('f')), blob_answer(limit, &padding, BASE64)).await;

        let failure = blob_of(&server, limit).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::TooLarge), "{failure:?}");
    }

    #[test]
    fn the_bound_holds_the_base64_and_its_line_breaks_and_saturates() {
        let content: Vec<u8> = vec![0; 4_500];
        let body =
            serde_json::to_vec(&blob_answer(content.len(), &encoded(&content), BASE64)).unwrap();
        assert!(
            body.len() <= encoded_bound(content.len()),
            "{} > {}",
            body.len(),
            encoded_bound(content.len())
        );
        assert!(encoded_bound(content.len()) < body.len() + ENVELOPE);
        assert_eq!(encoded_bound(0), ENVELOPE);
        assert_eq!(encoded_bound(usize::MAX), MAXIMUM_ANSWER_BYTES);
        assert_eq!(encoded_bound(MAXIMUM_ANSWER_BYTES), MAXIMUM_ANSWER_BYTES);
    }

    #[tokio::test]
    async fn a_blob_that_is_not_base64_is_unreadable() {
        for (content, encoding, size) in [
            ("aGVsbG8=", "utf-8", 5),
            ("hello", "utf-8", 5),
            ("aGVs!G8=", BASE64, 0),
            ("aGVsbG8", BASE64, 5),
            ("aGVs bG8=", BASE64, 5),
            ("aGVs\rbG8=", BASE64, 5),
        ] {
            let server = serving(blobs(&commit('f')), blob_answer(size, content, encoding)).await;

            let failure = blob_of(&server, 100).await.unwrap_err();

            assert!(
                is_unreadable(&failure),
                "{content:?} in {encoding} of size {size}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_blob_that_decodes_to_another_size_than_github_gave_is_unreadable() {
        for size in [4, 6] {
            let server = serving(
                blobs(&commit('f')),
                blob_answer(size, &encoded(b"hello"), BASE64),
            )
            .await;

            let failure = blob_of(&server, 100).await.unwrap_err();

            assert!(is_unreadable(&failure), "size {size}: {failure:?}");
        }
    }
}
