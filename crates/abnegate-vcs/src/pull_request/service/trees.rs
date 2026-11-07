use super::*;
use crate::blob::Blob;
use crate::object_id::ObjectId;
use crate::parse_error::ParseError;
use crate::pull_request::Tree;
use crate::pull_request::github_tree::GitHubTree;
use crate::pull_request::github_tree_entry::GitHubTreeEntry;
use crate::remote_branch_name::RemoteBranchName;
use crate::tree_entry::TreeEntry;

/// The query parameter that asks GitHub for every entry beneath a tree, and
/// the value that turns it on.
const RECURSIVE: (&str, &str) = ("recursive", "1");

/// Most bytes of a tree read, past the [`MAXIMUM_ANSWER_BYTES`] other answers
/// are held to. GitHub documents up to 100,000 entries of a recursive tree,
/// about 31 MiB at the density of a large repository, and long paths add more.
const MAXIMUM_TREE_BYTES: usize = 128 * 1024 * 1024;

impl PullRequestService {
    /// A git tree's entries, read without following the trees within it
    /// unless `recursive` asks to.
    ///
    /// `tree` names a tree, or a commit, which GitHub reads as the commit's
    /// tree, so [`ObjectId::from`] a commit's identifier starts a walk at its
    /// root. Read without recursing, each entry's path is its name within the
    /// tree, so a caller can walk a path one tree at a time and check every
    /// entry it passes on the way, as one refusing symbolic links and
    /// submodules must. [`Tree::truncated`] says whether GitHub cut the list
    /// short; a caller that needs every entry must not take a truncated tree
    /// for a whole one. The answer is read up to 128 MiB, past the bound on
    /// other answers, and a longer one is [`PullRequestError::GitHubApi`].
    ///
    /// A tree or entry whose object name is not a full one is
    /// [`PullRequestError::Parse`]; no part of the tree is returned.
    pub async fn fetch_tree(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tree: &ObjectId,
        recursive: bool,
    ) -> PullRequestResult<Tree> {
        let url = self.tree_endpoint(repository, tree.as_str(), recursive)?;
        self.tree(url, token).await
    }

    /// The git tree at `reference`, a branch, a tag, a commit, or `HEAD` for
    /// the tip of the default branch, read without following the trees within
    /// it unless `recursive` asks to.
    ///
    /// `reference` is held to the rules `git check-ref-format` holds a name
    /// under `refs/heads/` to, and is [`PullRequestError::Parse`] with
    /// [`ParseError::BranchName`] before anything is sent when it breaks
    /// them, or when it is `@`, git's shorthand for `HEAD`. A name only a
    /// command line would misread, such as `-dash`, which
    /// [`Self::list_branches`] can return, is read like any other, because
    /// it never reaches one. It travels as one path segment, so a `/` in it
    /// is sent as `%2F`. The answer is read as [`Self::fetch_tree`] reads
    /// one, [`Tree::truncated`] included.
    pub async fn fetch_tree_at(
        &self,
        repository: &Repository,
        token: &SecretValue,
        reference: &str,
        recursive: bool,
    ) -> PullRequestResult<Tree> {
        if RemoteBranchName::refuses_reference(reference) {
            return Err(ParseError::BranchName(reference.to_string()).into());
        }
        let url = self.tree_endpoint(repository, reference, recursive)?;
        self.tree(url, token).await
    }

    /// A git blob's bytes, whole, so long as there are no more than `limit`
    /// of them.
    ///
    /// GitHub is asked for the blob's bytes as they are, and its answer is
    /// read no further than `limit` bytes, the caller's bound on what it
    /// holds in memory. An answer longer than that is
    /// [`PullRequestError::TooLarge`] and is never returned in part; one
    /// whose declared length is longer is refused before any of it is read.
    pub async fn fetch_blob(
        &self,
        repository: &Repository,
        token: &SecretValue,
        blob: &ObjectId,
        limit: usize,
    ) -> PullRequestResult<Blob> {
        let url = self.repository_endpoint(repository, &["git", "blobs", blob.as_str()])?;

        let response = self
            .answered(self.request(Method::GET, url, token, RAW)?)
            .await?;
        let declared_past_limit = response
            .content_length()
            .is_some_and(|declared| u64::try_from(limit).is_ok_and(|limit| declared > limit));
        if declared_past_limit {
            return Err(PullRequestError::TooLarge);
        }
        let (content, more) = read_prefix(response, limit).await?;
        if more {
            return Err(PullRequestError::TooLarge);
        }
        Ok(Blob::new(blob.clone(), content))
    }

    /// Where `repository`'s tree at `tree` is read from, asking for every
    /// entry beneath it when `recursive` is set.
    fn tree_endpoint(
        &self,
        repository: &Repository,
        tree: &str,
        recursive: bool,
    ) -> PullRequestResult<Url> {
        let mut url = self.repository_endpoint(repository, &["git", "trees", tree])?;
        if recursive {
            let (name, value) = RECURSIVE;
            url.query_pairs_mut().append_pair(name, value);
        }
        Ok(url)
    }

    /// The tree GitHub answers `url` with, every object name parsed.
    async fn tree(&self, url: Url, token: &SecretValue) -> PullRequestResult<Tree> {
        let response = self
            .answered(self.request(Method::GET, url, token, ACCEPT)?)
            .await?;
        let answer: GitHubTree = decode_within(response, MAXIMUM_TREE_BYTES).await?;
        let entries = answer
            .tree
            .into_iter()
            .map(entry)
            .collect::<PullRequestResult<Vec<TreeEntry>>>()?;
        Ok(Tree::new(ObjectId::parse(&answer.sha)?, entries).with_truncated(answer.truncated))
    }
}

/// A tree entry GitHub listed, its object name parsed.
fn entry(listed: GitHubTreeEntry) -> PullRequestResult<TreeEntry> {
    let entry = TreeEntry::new(
        listed.path,
        listed.mode,
        listed.kind,
        ObjectId::parse(&listed.sha)?,
    );
    Ok(match listed.size {
        Some(size) => entry.with_size(size),
        None => entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::hanging_up;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use crate::tree_entry_kind::TreeEntryKind;
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

    fn object(digit: char) -> ObjectId {
        ObjectId::from(commit(digit))
    }

    fn trees(sha: &ObjectId) -> String {
        format!("/repos/acme/project/git/trees/{sha}")
    }

    fn blobs(sha: &ObjectId) -> String {
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

    async fn serving_bytes(content: &[u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(blobs(&object('f'))))
            .and(header("accept", RAW))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.to_vec()))
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
            .fetch_tree(&project(&service), &token(), &object('a'), recursive)
            .await
    }

    async fn blob_of(server: &MockServer, limit: usize) -> PullRequestResult<Blob> {
        let service = stand_in(server).await;
        service
            .fetch_blob(&project(&service), &token(), &object('f'), limit)
            .await
    }

    /// A blob answer that declares eleven bytes and then hangs up before
    /// sending any of them.
    const DECLARED_ELEVEN: &str =
        "HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\n";

    #[tokio::test]
    async fn a_blob_declared_past_the_limit_is_too_large_before_it_is_read() {
        let service = hanging_up(|_| DECLARED_ELEVEN).await;

        let failure = service
            .fetch_blob(&project(&service), &token(), &object('f'), 10)
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::TooLarge), "{failure:?}");
    }

    fn is_unreadable(failure: &PullRequestError) -> bool {
        matches!(failure, PullRequestError::GitHubApi(message) if message == UNREADABLE)
    }

    #[tokio::test]
    async fn a_tree_lists_each_entry_with_its_mode_and_kind() {
        let server = serving(trees(&object('a')), listing(false)).await;

        let tree = tree_of(&server, false).await.unwrap();

        assert_eq!(
            tree,
            Tree::new(
                object('a'),
                vec![
                    TreeEntry::new("README.md", "100644", TreeEntryKind::Blob, object('b'))
                        .with_size(30),
                    TreeEntry::new("latest", "120000", TreeEntryKind::Blob, object('c'))
                        .with_size(9),
                    TreeEntry::new("src", "040000", TreeEntryKind::Tree, object('d')),
                    TreeEntry::new("vendor", "160000", TreeEntryKind::Commit, object('e')),
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
            let server = serving(trees(&object('a')), listing(truncated)).await;

            let tree = tree_of(&server, false).await.unwrap();

            assert_eq!(tree.truncated, truncated);
            assert_eq!(tree.entries.len(), 4, "truncated {truncated}");
        }
    }

    #[tokio::test]
    async fn a_tree_is_read_recursively_only_when_asked() {
        for (recursive, query) in [(false, None), (true, Some("recursive=1"))] {
            let server = serving(trees(&object('a')), listing(false)).await;

            tree_of(&server, recursive).await.unwrap();

            let received = server.received_requests().await.unwrap();
            assert_eq!(received[0].url.query(), query, "recursive {recursive}");
        }
    }

    #[tokio::test]
    async fn a_tree_that_does_not_say_whether_it_was_cut_short_is_unreadable() {
        let mut answer = listing(false);
        answer.as_object_mut().unwrap().remove("truncated");
        let server = serving(trees(&object('a')), answer).await;

        let failure = tree_of(&server, false).await.unwrap_err();

        assert!(is_unreadable(&failure), "{failure:?}");
    }

    #[tokio::test]
    async fn a_tree_entry_with_a_malformed_sha_fails_the_whole_tree() {
        let mut answer = listing(false);
        answer["tree"][2]["sha"] = json!("not-a-sha");
        let server = serving(trees(&object('a')), answer).await;

        let failure = tree_of(&server, false).await.unwrap_err();

        assert!(
            matches!(&failure, PullRequestError::Parse(ParseError::ObjectId(value)) if value == "not-a-sha"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_blob_within_the_limit_is_returned_whole_byte_for_byte() {
        let content: Vec<u8> = (0..=u8::MAX).cycle().take(3_001).collect();
        assert!(std::str::from_utf8(&content).is_err(), "not text");
        let server = serving_bytes(&content).await;

        let blob = blob_of(&server, content.len()).await.unwrap();

        assert_eq!(blob, Blob::new(object('f'), content));
    }

    #[tokio::test]
    async fn an_empty_blob_is_read() {
        let server = serving_bytes(b"").await;

        let blob = blob_of(&server, 0).await.unwrap();

        assert_eq!(blob, Blob::new(object('f'), Vec::new()));
    }

    #[tokio::test]
    async fn a_blob_one_byte_over_the_limit_is_too_large() {
        let content = b"twelve bytes";
        let server = serving_bytes(content).await;

        let failure = blob_of(&server, content.len() - 1).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::TooLarge), "{failure:?}");
    }

    #[tokio::test]
    async fn a_blob_is_asked_for_as_its_raw_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(blobs(&object('f'))))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": "aGVsbG8=",
                "encoding": "base64",
            })))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(blobs(&object('f'))))
            .and(header("accept", RAW))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .expect(1)
            .mount(&server)
            .await;

        let blob = blob_of(&server, 5).await.unwrap();

        assert_eq!(blob.content, b"hello");
    }

    const REFERENCED: &str = "/repos/acme/project/git/trees";

    fn header_value<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn recursive() -> Vec<(&'static str, &'static str)> {
        vec![RECURSIVE]
    }

    async fn serving_at(
        reference: &str,
        query: Vec<(&'static str, &'static str)>,
        body: Value,
    ) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{REFERENCED}/{reference}")))
            .and(query_is(&query))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn tree_at(
        server: &MockServer,
        reference: &str,
        recursive: bool,
    ) -> PullRequestResult<Tree> {
        let service = stand_in(server).await;
        service
            .fetch_tree_at(&project(&service), &token(), reference, recursive)
            .await
    }

    fn nested() -> Value {
        json!({
            "sha": commit('a').as_str(),
            "tree": [
                {
                    "path": "README.md",
                    "mode": "100644",
                    "type": "blob",
                    "sha": commit('b').as_str(),
                    "size": 100,
                },
                {
                    "path": "src",
                    "mode": "040000",
                    "type": "tree",
                    "sha": commit('c').as_str(),
                },
                {
                    "path": "src/lib.rs",
                    "mode": "100644",
                    "type": "blob",
                    "sha": commit('d').as_str(),
                    "size": 500,
                },
            ],
            "truncated": false,
        })
    }

    fn nested_entries() -> Vec<TreeEntry> {
        vec![
            TreeEntry::new("README.md", "100644", TreeEntryKind::Blob, object('b')).with_size(100),
            TreeEntry::new("src", "040000", TreeEntryKind::Tree, object('c')),
            TreeEntry::new("src/lib.rs", "100644", TreeEntryKind::Blob, object('d')).with_size(500),
        ]
    }

    #[tokio::test]
    async fn a_branch_s_tree_is_read_recursively_with_every_entry_and_its_sha() {
        for branch in ["main", "develop"] {
            let server = serving_at(branch, recursive(), nested()).await;

            let tree = tree_at(&server, branch, true).await.unwrap();

            assert_eq!(tree, Tree::new(object('a'), nested_entries()), "{branch}");
            assert!(!tree.truncated, "{branch}");
            let received = server.received_requests().await.unwrap();
            assert_eq!(
                header_value(&received[0], "authorization"),
                Some("Bearer token"),
                "{branch}"
            );
        }
    }

    #[tokio::test]
    async fn a_tree_at_a_reference_is_read_without_recursing_unless_asked() {
        let server = serving_at("main", Vec::new(), nested()).await;

        tree_at(&server, "main", false).await.unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.query(), None);
    }

    #[tokio::test]
    async fn a_tree_at_a_reference_github_cut_short_says_so() {
        let mut answer = nested();
        answer["truncated"] = json!(true);
        let server = serving_at("main", recursive(), answer).await;

        let tree = tree_at(&server, "main", true).await.unwrap();

        assert!(tree.truncated);
        assert_eq!(tree.entries, nested_entries());
    }

    #[tokio::test]
    async fn an_empty_tree_at_a_reference_still_names_its_sha() {
        let answer = json!({
            "sha": commit('e').as_str(),
            "tree": [],
            "truncated": false,
        });
        let server = serving_at("main", recursive(), answer).await;

        let tree = tree_at(&server, "main", true).await.unwrap();

        assert_eq!(tree, Tree::new(object('e'), Vec::new()));
    }

    #[tokio::test]
    async fn an_anonymous_tree_read_carries_no_authorization() {
        let server = serving_at("main", recursive(), nested()).await;
        let service = stand_in(&server).await.with_anonymous_reads();

        let tree = service
            .fetch_tree_at(&project(&service), &SecretValue::new(""), "main", true)
            .await
            .unwrap();

        assert_eq!(tree.entries, nested_entries());
        let received = server.received_requests().await.unwrap();
        assert!(
            !received[0].headers.contains_key(AUTHORIZATION),
            "an anonymous read carries no authorization"
        );
    }

    #[tokio::test]
    async fn a_reference_github_holds_no_tree_for_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{REFERENCED}/missing")))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let failure = tree_at(&server, "missing", true).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_reference_git_would_refuse_is_refused_unsent() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        for reference in ["", "a..b", "@"] {
            let failure = tree_at(&server, reference, true).await.unwrap_err();

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
    async fn the_tip_of_the_default_branch_is_read_as_head() {
        let server = serving_at("HEAD", recursive(), nested()).await;

        let tree = tree_at(&server, "HEAD", true).await.unwrap();

        assert_eq!(tree.entries, nested_entries());
    }

    #[tokio::test]
    async fn a_reference_with_a_slash_travels_as_one_segment() {
        let server = serving_at("feature%2Fx", recursive(), nested()).await;

        let tree = tree_at(&server, "feature/x", true).await.unwrap();

        assert_eq!(tree.sha, object('a'));
    }

    #[tokio::test]
    async fn a_tree_answered_with_a_short_sha_is_refused() {
        let mut answer = nested();
        answer["sha"] = json!("abc123");
        let server = serving_at("main", recursive(), answer).await;

        let failure = tree_at(&server, "main", true).await.unwrap_err();

        assert!(
            matches!(&failure, PullRequestError::Parse(ParseError::ObjectId(value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tree_past_the_bound_on_other_answers_is_read_by_sha_and_by_reference() {
        let mut padded = nested().to_string();
        padded.push_str(&" ".repeat(MAXIMUM_ANSWER_BYTES + 1 - padded.len()));
        assert_eq!(padded.len(), MAXIMUM_ANSWER_BYTES + 1);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(padded))
            .expect(2)
            .mount(&server)
            .await;

        let by_sha = tree_of(&server, true).await.unwrap();
        let by_reference = tree_at(&server, "main", true).await.unwrap();

        assert_eq!(by_sha.entries, nested_entries());
        assert_eq!(by_reference.entries, nested_entries());
    }

    #[tokio::test]
    async fn a_tree_declared_past_its_own_bound_is_refused_before_it_is_read() {
        let service = hanging_up(|_| {
            Box::leak(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    MAXIMUM_TREE_BYTES + 1
                )
                .into_boxed_str(),
            )
        })
        .await;

        let failure = service
            .fetch_tree(&project(&service), &token(), &object('a'), true)
            .await
            .unwrap_err();

        assert!(
            matches!(&failure, PullRequestError::GitHubApi(text) if text == OVERSIZED),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_branch_led_by_a_dash_is_read_as_github_lists_it() {
        let server = serving_at("-dash", recursive(), nested()).await;

        let tree = tree_at(&server, "-dash", true).await.unwrap();

        assert_eq!(tree.entries, nested_entries());
    }
}
