use super::*;
use crate::gitlab::gitlab_tree_entry::GitLabTreeEntry;
use crate::object_id::ObjectId;
use crate::parse_error::ParseError;
use crate::tag_name::TagName;
use crate::tree_entry::TreeEntry;
use reqwest::header::LINK;
use std::collections::HashSet;

/// The query parameter naming the branch, tag or commit a tree is read at.
const REFERENCE: &str = "ref";

/// The query parameter and value that ask GitLab for every entry beneath the
/// tree, not only its own.
const RECURSIVE: (&str, &str) = ("recursive", "true");

/// The query parameter and value that ask GitLab for its largest page.
const PAGE_SIZE: (&str, &str) = ("per_page", "100");

/// The query parameter naming the directory a tree is read from.
const PATH: &str = "path";

/// The link relation that names a page's successor.
const NEXT: &str = "next";

/// The link parameter that names a link's relations.
const RELATION: &str = "rel";

/// What a link to a tree's next page off the configured origin is reported
/// as; it is never followed, so the token never travels there.
const OFF_ORIGIN: &str = "GitLab linked a tree's next page off the configured origin";

/// What a link to a tree's next page already followed is reported as, rather
/// than walked again forever.
const REPEATED: &str = "GitLab linked a tree's next page to one already read";

impl GitLabService {
    /// Every entry of the project's tree at `reference`, a branch, tag or
    /// commit, read recursively, beneath `path` when one is given and not
    /// empty.
    ///
    /// GitLab answers a hundred entries a page, and every page its `Link`
    /// header names as the next is read in turn, with no cap on how many. A
    /// next link that leaves the configured origin, or names a page already
    /// read, is [`GitLabError::GitLabApi`], and is never followed. GitLab
    /// reports no size for an entry, so each has none.
    ///
    /// `reference` is held to the rules git holds a ref's name to, `HEAD`
    /// aside, and `@` is refused: one that breaks them is
    /// [`GitLabError::Parse`], and nothing is sent. An entry whose object
    /// name is not a full one is [`GitLabError::Parse`] too, and no part of
    /// the tree is returned.
    pub async fn fetch_tree(
        &self,
        project: &Project,
        token: &SecretValue,
        reference: &str,
        path: Option<&str>,
    ) -> GitLabResult<Vec<TreeEntry>> {
        if !TagName::accepts_reference(reference) {
            return Err(ParseError::BranchName(reference.to_string()).into());
        }
        let mut url = self.project_endpoint(project, &["repository", "tree"]);
        {
            let mut query = url.query_pairs_mut();
            let (recursive, all) = RECURSIVE;
            let (page_size, most) = PAGE_SIZE;
            query
                .append_pair(REFERENCE, reference)
                .append_pair(recursive, all)
                .append_pair(page_size, most);
            if let Some(path) = path.filter(|path| !path.is_empty()) {
                query.append_pair(PATH, path);
            }
        }

        let mut entries = Vec::new();
        let mut visited = HashSet::from([url.clone()]);
        let mut page = Some(url);
        while let Some(url) = page.take() {
            let response = self.answered(self.request(url, token)?).await?;
            let next = next_page(&response)?;
            let listed: Vec<GitLabTreeEntry> = decode(response).await?;
            for listed in listed {
                entries.push(entry(listed)?);
            }
            page = match next {
                Some(next) if !self.holds(&next) => {
                    return Err(GitLabError::GitLabApi(OFF_ORIGIN.to_string()));
                }
                Some(next) if !visited.insert(next.clone()) => {
                    return Err(GitLabError::GitLabApi(REPEATED.to_string()));
                }
                next => next,
            };
        }
        Ok(entries)
    }
}

/// The page `response`'s `Link` header names as the next, resolved against
/// where it came from, or `None` on the last page. A header or link this
/// crate cannot read is [`GitLabError::GitLabApi`].
fn next_page(response: &Response) -> GitLabResult<Option<Url>> {
    let unreadable = || GitLabError::GitLabApi(UNREADABLE.to_string());
    for value in response.headers().get_all(LINK) {
        let value = value.to_str().map_err(|_| unreadable())?;
        for link in value.split(',') {
            let mut parts = link.split(';');
            let target = parts.next().unwrap_or_default().trim();
            if !parts.any(names_next) {
                continue;
            }
            let target = target
                .strip_prefix('<')
                .and_then(|target| target.strip_suffix('>'))
                .ok_or_else(unreadable)?;
            return response
                .url()
                .join(target)
                .map(Some)
                .map_err(|_| unreadable());
        }
    }
    Ok(None)
}

/// Whether a link's `parameter` names it the next page.
fn names_next(parameter: &str) -> bool {
    parameter.split_once('=').is_some_and(|(name, value)| {
        name.trim().eq_ignore_ascii_case(RELATION)
            && value
                .trim()
                .trim_matches('"')
                .split_ascii_whitespace()
                .any(|relation| relation.eq_ignore_ascii_case(NEXT))
    })
}

/// A tree entry GitLab listed, its object name parsed.
fn entry(listed: GitLabTreeEntry) -> GitLabResult<TreeEntry> {
    Ok(TreeEntry::new(
        listed.path,
        listed.mode,
        listed.kind,
        ObjectId::parse(&listed.id)?,
    ))
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
    use crate::tree_entry_kind::TreeEntryKind;
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
    use wiremock::matchers::query_param_is_missing;

    fn route() -> String {
        format!("{API}/projects/acme%2Fproject/repository/tree")
    }

    fn listed(path: &str, kind: &str, mode: &str, digit: char) -> Value {
        json!({
            "id": object(digit).as_str(),
            "name": path.rsplit('/').next().unwrap(),
            "type": kind,
            "path": path,
            "mode": mode,
        })
    }

    fn listing() -> Value {
        json!([
            listed("README.md", "blob", "100644", 'a'),
            listed("src", "tree", "040000", 'b'),
            listed("vendor", "commit", "160000", 'c'),
            listed("odd", "unheard-of", "100644", 'd'),
        ])
    }

    async fn serving(body: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    async fn tree_at(
        server: &MockServer,
        reference: &str,
        path: Option<&str>,
    ) -> GitLabResult<Vec<TreeEntry>> {
        served(server)
            .fetch_tree(&project(), &token(), reference, path)
            .await
    }

    async fn queries(server: &MockServer) -> Vec<Vec<(String, String)>> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.query_pairs().into_owned().collect())
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn page_two(server: &MockServer) -> String {
        format!(
            "{}{}?ref=main&recursive=true&per_page=100&page=2",
            server.uri(),
            route()
        )
    }

    #[tokio::test]
    async fn every_entry_is_listed_with_its_mode_kind_and_object() {
        let server = serving(listing()).await;

        let entries = tree_at(&server, "main", None).await.unwrap();

        assert_eq!(
            entries,
            [
                TreeEntry::new("README.md", "100644", TreeEntryKind::Blob, object('a')),
                TreeEntry::new("src", "040000", TreeEntryKind::Tree, object('b')),
                TreeEntry::new("vendor", "160000", TreeEntryKind::Commit, object('c')),
                TreeEntry::new("odd", "100644", TreeEntryKind::Unknown, object('d')),
            ]
        );
        assert!(entries.iter().all(|entry| entry.size.is_none()));
    }

    #[tokio::test]
    async fn a_single_file_tree_is_read_at_its_branch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .and(query_param(REFERENCE, "main"))
            .and(header("private-token", "token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed(
                "README.md",
                "blob",
                "100644",
                'e'
            )])))
            .expect(1)
            .mount(&server)
            .await;

        let entries = tree_at(&server, "main", None).await.unwrap();

        assert_eq!(
            entries,
            [TreeEntry::new(
                "README.md",
                "100644",
                TreeEntryKind::Blob,
                object('e')
            )]
        );
        assert!(entries[0].regular_file());
    }

    #[tokio::test]
    async fn the_tree_is_always_asked_for_recursively_a_hundred_entries_a_page() {
        let server = serving(json!([])).await;

        tree_at(&server, "main", None).await.unwrap();

        assert_eq!(
            queries(&server).await,
            [pairs(&[
                ("ref", "main"),
                ("recursive", "true"),
                ("per_page", "100"),
            ])]
        );
    }

    #[tokio::test]
    async fn a_path_is_sent_only_when_one_is_given_and_not_empty() {
        for (given, expected) in [
            (Some("src/lib"), Some("src/lib")),
            (Some(""), None),
            (None, None),
        ] {
            let server = serving(json!([])).await;

            tree_at(&server, "main", given).await.unwrap();

            let sent = queries(&server).await.remove(0);
            let path = sent
                .iter()
                .find(|(name, _)| name == PATH)
                .map(|(_, value)| value.as_str());
            assert_eq!(path, expected, "{given:?}");
        }
    }

    #[tokio::test]
    async fn a_branch_with_a_slash_travels_whole_as_the_ref() {
        let server = serving(listing()).await;

        tree_at(&server, "feature/my-branch", None).await.unwrap();

        let received = server.received_requests().await.unwrap();
        assert_eq!(
            received[0].url.query(),
            Some("ref=feature%2Fmy-branch&recursive=true&per_page=100")
        );
    }

    #[tokio::test]
    async fn a_branch_with_query_characters_adds_no_parameter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .and(query_param(REFERENCE, "fix/bug-123&feature=new"))
            .and(query_param_is_missing("feature"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([listed("file.txt", "blob", "100644", 'f')])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let entries = tree_at(&server, "fix/bug-123&feature=new", None)
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            queries(&server).await,
            [pairs(&[
                ("ref", "fix/bug-123&feature=new"),
                ("recursive", "true"),
                ("per_page", "100"),
            ])]
        );
    }

    #[tokio::test]
    async fn every_page_the_link_header_names_next_is_read_in_turn() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .and(query_param_is_missing("page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "link",
                        format!(
                            "<{}>; rel=\"next\", <{}>; rel=\"last\"",
                            page_two(&server),
                            page_two(&server)
                        ),
                    )
                    .set_body_json(json!([listed("README.md", "blob", "100644", 'a')])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(route()))
            .and(query_param("page", "2"))
            .and(header("private-token", "token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", format!("<{}>; rel=\"first\"", page_two(&server)))
                    .set_body_json(json!([listed("src/lib.rs", "blob", "100644", 'b')])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let entries = tree_at(&server, "main", None).await.unwrap();

        let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, ["README.md", "src/lib.rs"]);
    }

    #[tokio::test]
    async fn a_next_page_off_the_origin_is_refused_and_never_reached() {
        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", format!("<{}>; rel=\"next\"", page_two(&elsewhere)))
                    .set_body_json(listing()),
            )
            .mount(&server)
            .await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == OFF_ORIGIN),
            "{failure:?}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_next_page_already_read_is_refused_rather_than_walked_forever() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", format!("<{}>; rel=\"next\"", page_two(&server)))
                    .set_body_json(listing()),
            )
            .mount(&server)
            .await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == REPEATED),
            "{failure:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_next_page_that_names_the_first_is_refused() {
        let server = MockServer::start().await;
        let first = format!(
            "{}{}?ref=main&recursive=true&per_page=100",
            server.uri(),
            route()
        );
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", format!("<{first}>; rel=\"next\""))
                    .set_body_json(listing()),
            )
            .mount(&server)
            .await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == REPEATED),
            "{failure:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_next_link_that_cannot_be_read_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("link", "page-two; rel=\"next\"")
                    .set_body_json(listing()),
            )
            .mount(&server)
            .await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::GitLabApi(reason) if reason == UNREADABLE),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn an_entry_with_a_short_object_name_fails_the_whole_tree() {
        let mut answer = listing();
        answer[1]["id"] = json!("abc123");
        let server = serving(answer).await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::Parse(ParseError::ObjectId(value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_missing_project_or_ref_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route()))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let failure = tree_at(&server, "main", None).await.unwrap_err();

        assert!(matches!(failure, GitLabError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_reference_git_would_refuse_is_refused_unsent() {
        let server = serving(listing()).await;

        for refused in ["", "a..b", "@", "a b", "-x", "refs/heads/x.lock"] {
            let failure = tree_at(&server, refused, None).await.unwrap_err();

            assert!(
                matches!(&failure, GitLabError::Parse(ParseError::BranchName(value)) if value == refused),
                "{refused:?}: {failure:?}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn head_and_any_ref_git_accepts_are_sent() {
        let commit = object('a');
        for accepted in ["HEAD", "main", "v1.2.3", "+release", commit.as_str()] {
            let server = serving(listing()).await;

            tree_at(&server, accepted, None).await.unwrap();

            assert_eq!(
                queries(&server).await[0][0],
                ("ref".to_string(), accepted.to_string())
            );
        }
    }

    #[tokio::test]
    async fn an_anonymous_tree_read_sends_no_token() {
        let server = serving(listing()).await;

        served(&server)
            .with_anonymous_reads()
            .fetch_tree(&project(), &empty(), "main", None)
            .await
            .unwrap();

        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("private-token"));
    }

    #[test]
    fn only_a_link_whose_relations_include_next_is_the_next_page() {
        for (parameter, next) in [
            ("rel=\"next\"", true),
            (" rel=next", true),
            ("REL=\"Next\"", true),
            ("rel=\"prev next\"", true),
            ("rel=\"prev\"", false),
            ("rel=\"nextpage\"", false),
            ("title=\"next\"", false),
            ("next", false),
        ] {
            assert_eq!(names_next(parameter), next, "{parameter}");
        }
    }
}
