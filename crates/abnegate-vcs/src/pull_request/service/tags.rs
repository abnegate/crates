use super::*;
use crate::object_id::ObjectId;
use crate::parse_error::ParseError;
use crate::pull_request::GitReference;
use crate::pull_request::Page;
use crate::pull_request::Tag;
use crate::pull_request::TagRequest;
use crate::pull_request::github_object::GitHubObject;
use crate::pull_request::github_reference::GitHubReference;
use crate::pull_request::github_reference_request::GitHubReferenceRequest;
use crate::pull_request::github_tag::GitHubTag;
use crate::pull_request::github_tag_request::GitHubTagRequest;
use crate::tag_name::TagName;

/// What every full git reference name starts with.
const REFERENCES: &str = "refs/";

/// What every full tag reference name starts with.
const TAG_REFERENCES: &str = "refs/tags/";

/// What separates the components of a git reference name.
const COMPONENT: char = '/';

/// What GitHub says when the reference asked to be created is already there.
const REFERENCE_EXISTS: &str = "Reference already exists";

impl PullRequestService {
    /// Page `page` of `repository`'s tags, in the order GitHub lists them,
    /// at `per_page` rows a page up to GitHub's hundred.
    ///
    /// A row that leaves out a tag's name or commit is
    /// [`PullRequestError::GitHubApi`], and one whose commit is not a full
    /// identifier is [`PullRequestError::Parse`], never a page without it.
    pub async fn list_tags(
        &self,
        repository: &Repository,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Tag>> {
        let asked = asked(per_page);
        let url = self.repository_endpoint(repository, &["tags"])?;
        let rows: Vec<GitHubTag> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let tags = rows
            .into_iter()
            .map(GitHubTag::tag)
            .collect::<PullRequestResult<Vec<Tag>>>()?;
        Ok(Page::after(tags, page, read, asked))
    }

    /// The reference `repository` holds for the tag `tag`, exactly as named,
    /// or `None` when it holds none.
    ///
    /// The reference points at the tag's annotated tag object, or at the
    /// commit itself when the tag is lightweight, as [`GitReference::kind`]
    /// says. The tag is held to git's own rules for a tag name, and one git
    /// refuses is [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent. Each
    /// of its `/`-separated components travels as its own path segment, so
    /// `pkg/v1.2.3` is asked for as `git/ref/tags/pkg/v1.2.3`, as GitHub's
    /// references API reads it. GitHub answers a tag it does not hold with a
    /// 404, and a repository the token cannot see with the same 404, so both
    /// read as `None`. An answer that names the object by anything but a full
    /// object name is [`PullRequestError::Parse`].
    pub async fn fetch_tag_reference(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tag: &str,
    ) -> PullRequestResult<Option<GitReference>> {
        let tag = TagName::parse(tag)?;
        let segments: Vec<&str> = ["git", "ref", "tags"]
            .into_iter()
            .chain(tag.components())
            .collect();
        let url = self.repository_endpoint(repository, &segments)?;

        let response = self
            .sent(self.request(Method::GET, url, token, ACCEPT)?)
            .await?;
        match response.status() {
            status if status.is_success() => {
                let reference: GitHubReference = decode(response).await?;
                reference.reference().map(Some)
            }
            StatusCode::NOT_FOUND => Ok(None),
            _ => Err(refusal(response).await),
        }
    }

    /// Whether `repository` holds the tag `tag`, exactly as named: whether
    /// [`Self::fetch_tag_reference`] finds a reference for it, held to the
    /// same rules and answered the same way.
    pub async fn tag_exists(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tag: &str,
    ) -> PullRequestResult<bool> {
        Ok(self
            .fetch_tag_reference(repository, token, tag)
            .await?
            .is_some())
    }

    /// Create an annotated tag object for `request`, and the name GitHub gave
    /// it.
    ///
    /// The object alone names no tag: pass the name this returns to
    /// [`Self::create_tag_reference`] with the tag's name to publish it. The
    /// tag is held to git's own rules for a tag name, and one git refuses is
    /// [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent. An
    /// answer that names the object by anything but a full object name is
    /// [`PullRequestError::Parse`] too.
    pub async fn create_tag(
        &self,
        repository: &Repository,
        token: &SecretValue,
        request: &TagRequest,
    ) -> PullRequestResult<ObjectId> {
        let tag = TagName::parse(&request.tag)?;
        let url = self.repository_endpoint(repository, &["git", "tags"])?;
        let body = GitHubTagRequest::under(&tag, request);

        let created: GitHubObject = self
            .exchange(self.request(Method::POST, url, token, ACCEPT)?.json(&body))
            .await?;
        Ok(ObjectId::parse(&created.sha)?)
    }

    /// Publish the tag `tag` as the reference `refs/tags/<tag>`, pointing at
    /// `sha`: an annotated tag object [`Self::create_tag`] made, or a commit
    /// for a lightweight tag.
    ///
    /// The tag is held to git's own rules for a tag name, and one git refuses
    /// is [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent. A
    /// tag that already exists is [`PullRequestError::ReferenceExists`],
    /// naming its full reference, whatever it points at.
    pub async fn create_tag_reference(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tag: &str,
        sha: &ObjectId,
    ) -> PullRequestResult<()> {
        let tag = TagName::parse(tag)?;
        let reference = format!("{TAG_REFERENCES}{}", tag.as_str());
        self.create_reference(repository, token, &reference, sha)
            .await
    }

    /// Create the git reference `reference`, such as `refs/tags/v1.2.0`,
    /// pointing at `sha`.
    ///
    /// `reference` is the full name: under `refs/`, with a category such as
    /// `tags/` or `heads/` beneath it, and accepted by
    /// `git check-ref-format`; under `refs/tags/`, what follows is held to
    /// git's own rules for a tag name too. Any other is
    /// [`PullRequestError::Parse`] with
    /// [`ParseError::Reference`](crate::ParseError::Reference), and nothing is
    /// sent. A reference that already exists is
    /// [`PullRequestError::ReferenceExists`], whatever it points at.
    pub async fn create_reference(
        &self,
        repository: &Repository,
        token: &SecretValue,
        reference: &str,
        sha: &ObjectId,
    ) -> PullRequestResult<()> {
        if !full_reference(reference) {
            return Err(ParseError::Reference(reference.to_string()).into());
        }
        let url = self.repository_endpoint(repository, &["git", "refs"])?;
        let body = GitHubReferenceRequest {
            reference,
            sha: sha.as_str(),
        };

        let response = self
            .sent(self.request(Method::POST, url, token, ACCEPT)?.json(&body))
            .await?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let refusal = explained(response).await?;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(REFERENCE_EXISTS) {
            return Err(PullRequestError::ReferenceExists(reference.to_string()));
        }
        Err(unexpected(status, &refusal))
    }
}

/// Whether `reference` is a full git reference name: under `refs/`, naming a
/// category and a name beneath it, and one `git check-ref-format` accepts;
/// under `refs/tags/`, naming a tag git accepts.
fn full_reference(reference: &str) -> bool {
    let named = reference
        .strip_prefix(REFERENCES)
        .is_some_and(|beneath| beneath.contains(COMPONENT) && !TagName::refuses(beneath));
    let tagged = reference
        .strip_prefix(TAG_REFERENCES)
        .is_none_or(|tag| TagName::parse(tag).is_ok());
    named && tagged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::ObjectKind;
    use crate::pull_request::Tagger;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::enterprise;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    const TAGS: &str = "/repos/acme/project/tags";

    const GIT_TAGS: &str = "/repos/acme/project/git/tags";

    const GIT_REFERENCES: &str = "/repos/acme/project/git/refs";

    const TAG_REFERENCE: &str = "/repos/acme/project/git/ref/tags";

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    fn object(digit: char) -> ObjectId {
        ObjectId::from(commit(digit))
    }

    fn row(name: &str, digit: char) -> Value {
        json!({
            "name": name,
            "commit": {
                "sha": commit(digit).as_str(),
                "url": format!("https://api.github.com/repos/acme/project/commits/{digit}"),
            },
            "zipball_url": "https://api.github.com/repos/acme/project/zipball/v1",
        })
    }

    /// A mock that answers anything, and that no request may reach.
    async fn untouched(server: &MockServer) {
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(server)
            .await;
    }

    async fn listing(server: &MockServer, route: &str, asked: &str, listed: Vec<Value>) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(query_is(&[("per_page", asked), ("page", "2")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn list(service: &PullRequestService, per_page: u8) -> PullRequestResult<Page<Tag>> {
        service
            .list_tags(&project(service), &token(), page(2), rows(per_page))
            .await
    }

    #[tokio::test]
    async fn a_full_page_of_tags_is_read_and_followed() {
        let server = MockServer::start().await;
        listing(
            &server,
            TAGS,
            "2",
            vec![row("v1.1.0", 'a'), row("pkg/v1.0.0", 'b')],
        )
        .await;
        let service = stand_in(&server).await;

        let listed = list(&service, 2).await.unwrap();

        assert_eq!(
            listed,
            Page::new(
                vec![
                    Tag::new("v1.1.0", commit('a')),
                    Tag::new("pkg/v1.0.0", commit('b')),
                ],
                Some(page(3))
            )
        );
    }

    #[tokio::test]
    async fn a_short_page_of_tags_is_the_last_and_never_asks_past_a_hundred() {
        let server = MockServer::start().await;
        listing(&server, TAGS, "100", vec![row("v1.0.0", 'a')]).await;
        let service = stand_in(&server).await;

        let listed = list(&service, u8::MAX).await.unwrap();

        assert_eq!(
            listed,
            Page::new(vec![Tag::new("v1.0.0", commit('a'))], None)
        );
    }

    #[tokio::test]
    async fn a_tag_row_missing_its_name_or_commit_fails_the_whole_page() {
        for required in ["name", "commit"] {
            let mut broken = row("v1.0.0", 'b');
            broken.as_object_mut().unwrap().remove(required);
            let server = MockServer::start().await;
            listing(&server, TAGS, "5", vec![row("v1.1.0", 'a'), broken]).await;
            let service = stand_in(&server).await;

            let failure = list(&service, 5).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{required} missing: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tag_on_anything_but_a_full_commit_fails_the_page() {
        let mut abbreviated = row("v1.0.0", 'b');
        abbreviated["commit"]["sha"] = json!("abc123");
        let server = MockServer::start().await;
        listing(&server, TAGS, "5", vec![abbreviated]).await;
        let service = stand_in(&server).await;

        let failure = list(&service, 5).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::CommitSha(ref value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn tags_are_listed_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        listing(
            &server,
            &format!("/api/v3{TAGS}"),
            "5",
            vec![row("v1.0.0", 'a')],
        )
        .await;
        let service = enterprise(&server).await;

        let listed = list(&service, 5).await.unwrap();

        assert_eq!(listed.items, [Tag::new("v1.0.0", commit('a'))]);
    }

    async fn reference_answering(server: &MockServer, route: String, status: u16) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "ref": "refs/tags/v1.2.3",
                "object": { "sha": commit('a').as_str(), "type": "tag" },
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn exists(service: &PullRequestService, tag: &str) -> PullRequestResult<bool> {
        service.tag_exists(&project(service), &token(), tag).await
    }

    #[tokio::test]
    async fn a_tag_the_repository_holds_exists() {
        let server = MockServer::start().await;
        reference_answering(&server, format!("{TAG_REFERENCE}/v1.2.3"), 200).await;
        let service = stand_in(&server).await;

        assert!(exists(&service, "v1.2.3").await.unwrap());
    }

    async fn reference_of(
        server: &MockServer,
        answer: Value,
    ) -> PullRequestResult<Option<GitReference>> {
        Mock::given(method("GET"))
            .and(path(format!("{TAG_REFERENCE}/pkg/v1.2.3")))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(1)
            .mount(server)
            .await;
        let service = stand_in(server).await;
        service
            .fetch_tag_reference(&project(&service), &token(), "pkg/v1.2.3")
            .await
    }

    #[tokio::test]
    async fn a_tag_s_reference_names_the_object_it_points_at_and_its_kind() {
        for (word, kind) in [
            ("tag", ObjectKind::Tag),
            ("commit", ObjectKind::Commit),
            ("submodule", ObjectKind::Unknown),
        ] {
            let server = MockServer::start().await;

            let reference = reference_of(
                &server,
                json!({
                    "ref": "refs/tags/pkg/v1.2.3",
                    "object": { "sha": commit('a').as_str(), "type": word },
                }),
            )
            .await
            .unwrap();

            assert_eq!(
                reference,
                Some(GitReference::new(object('a'), kind)),
                "{word}"
            );
        }
    }

    #[tokio::test]
    async fn a_tag_s_reference_that_names_no_kind_is_of_an_unknown_kind() {
        let server = MockServer::start().await;

        let reference = reference_of(
            &server,
            json!({ "ref": "refs/tags/pkg/v1.2.3", "object": { "sha": commit('a').as_str() } }),
        )
        .await
        .unwrap();

        assert_eq!(
            reference,
            Some(GitReference::new(object('a'), ObjectKind::Unknown))
        );
    }

    #[tokio::test]
    async fn a_tag_s_reference_to_anything_but_a_full_object_name_is_refused() {
        let server = MockServer::start().await;

        let failure = reference_of(
            &server,
            json!({ "ref": "refs/tags/pkg/v1.2.3", "object": { "sha": "abc123", "type": "tag" } }),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::ObjectId(ref value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_s_reference_github_cannot_find_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{TAG_REFERENCE}/v9.9.9")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let reference = service
            .fetch_tag_reference(&project(&service), &token(), "v9.9.9")
            .await
            .unwrap();

        assert_eq!(reference, None);
    }

    #[tokio::test]
    async fn a_tag_s_reference_is_read_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        reference_answering(&server, format!("/api/v3{TAG_REFERENCE}/pkg/v1.2.3"), 200).await;
        let service = enterprise(&server).await;

        let reference = service
            .fetch_tag_reference(&project(&service), &token(), "pkg/v1.2.3")
            .await
            .unwrap();

        assert_eq!(
            reference,
            Some(GitReference::new(object('a'), ObjectKind::Tag))
        );
    }

    #[tokio::test]
    async fn a_tag_github_answers_with_not_found_does_not_exist() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{TAG_REFERENCE}/v9.9.9")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        assert!(!exists(&service, "v9.9.9").await.unwrap());
    }

    #[tokio::test]
    async fn a_slashed_tag_is_asked_for_as_literal_path_segments() {
        for (tag, sent) in [
            ("pkg/v1.2.3", "pkg/v1.2.3"),
            ("team/pkg/v2", "team/pkg/v2"),
            ("v1.0.0+build", "v1.0.0+build"),
        ] {
            let server = MockServer::start().await;
            reference_answering(&server, format!("{TAG_REFERENCE}/{sent}"), 200).await;
            let service = stand_in(&server).await;

            assert!(exists(&service, tag).await.unwrap(), "{tag}");
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests[0].url.path(),
                format!("{TAG_REFERENCE}/{sent}"),
                "{tag} is never sent with its slash encoded"
            );
        }
    }

    #[tokio::test]
    async fn a_tag_git_refuses_is_refused_before_asking() {
        for tag in ["", "..", "pkg//v1", "v1/", "-v1", "HEAD", "v1?x", "v 1"] {
            let server = MockServer::start().await;
            untouched(&server).await;
            let service = stand_in(&server).await;

            let failure = exists(&service, tag).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::Tag(ref refused)) if refused == tag),
                "{tag:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tag_lookup_github_refuses_otherwise_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{TAG_REFERENCE}/v1.2.3")))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_json(json!({ "message": "Git Repository is empty." })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = exists(&service, "v1.2.3").await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Refused { status: 409, ref message } if message == "Git Repository is empty."),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_is_looked_up_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        reference_answering(&server, format!("/api/v3{TAG_REFERENCE}/pkg/v1.2.3"), 200).await;
        let service = enterprise(&server).await;

        assert!(exists(&service, "pkg/v1.2.3").await.unwrap());
    }

    fn tagging() -> TagRequest {
        TagRequest::new(
            "v1.2.0",
            "Version 1.2.0",
            commit('a'),
            Tagger::new("Release Bot", "release@example.com"),
        )
    }

    async fn tag_creation(server: &MockServer, route: String, status: u16, answer: Value) {
        Mock::given(method("POST"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({
                "tag": "v1.2.0",
                "message": "Version 1.2.0",
                "object": commit('a').as_str(),
                "type": "commit",
                "tagger": { "name": "Release Bot", "email": "release@example.com" },
            })))
            .respond_with(ResponseTemplate::new(status).set_body_json(answer))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn create_tag(
        service: &PullRequestService,
        request: &TagRequest,
    ) -> PullRequestResult<ObjectId> {
        service
            .create_tag(&project(service), &token(), request)
            .await
    }

    #[tokio::test]
    async fn an_annotated_tag_is_created_on_its_commit_and_named_by_github() {
        let server = MockServer::start().await;
        tag_creation(
            &server,
            GIT_TAGS.to_string(),
            201,
            json!({ "sha": commit('f').as_str(), "tag": "v1.2.0" }),
        )
        .await;
        let service = stand_in(&server).await;

        let created = create_tag(&service, &tagging()).await.unwrap();

        assert_eq!(created, object('f'));
    }

    #[tokio::test]
    async fn a_tag_object_github_names_by_anything_but_a_full_name_is_refused() {
        let server = MockServer::start().await;
        tag_creation(
            &server,
            GIT_TAGS.to_string(),
            201,
            json!({ "sha": "abc123" }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create_tag(&service, &tagging()).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::ObjectId(ref value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_github_will_not_create_is_refused_with_its_words() {
        let server = MockServer::start().await;
        tag_creation(
            &server,
            GIT_TAGS.to_string(),
            422,
            json!({ "message": "Object does not exist" }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create_tag(&service, &tagging()).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Refused { status: 422, ref message } if message == "Object does not exist"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_object_git_would_refuse_is_never_sent() {
        let server = MockServer::start().await;
        untouched(&server).await;
        let service = stand_in(&server).await;
        let mut request = tagging();
        request.tag = "v1..2".to_string();

        let failure = create_tag(&service, &request).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::Tag(ref refused)) if refused == "v1..2"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_object_is_created_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        tag_creation(
            &server,
            format!("/api/v3{GIT_TAGS}"),
            201,
            json!({ "sha": commit('f').as_str() }),
        )
        .await;
        let service = enterprise(&server).await;

        assert_eq!(create_tag(&service, &tagging()).await.unwrap(), object('f'));
    }

    async fn reference_creation(server: &MockServer, route: String, status: u16, answer: Value) {
        Mock::given(method("POST"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({
                "ref": "refs/tags/pkg/v1.2.0",
                "sha": commit('f').as_str(),
            })))
            .respond_with(ResponseTemplate::new(status).set_body_json(answer))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn create_reference(
        service: &PullRequestService,
        reference: &str,
    ) -> PullRequestResult<()> {
        service
            .create_reference(&project(service), &token(), reference, &object('f'))
            .await
    }

    #[tokio::test]
    async fn a_reference_is_created_pointing_at_its_object() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            GIT_REFERENCES.to_string(),
            201,
            json!({ "ref": "refs/tags/pkg/v1.2.0" }),
        )
        .await;
        let service = stand_in(&server).await;

        create_reference(&service, "refs/tags/pkg/v1.2.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_reference_that_already_exists_is_reported_as_existing() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            GIT_REFERENCES.to_string(),
            422,
            json!({ "message": "Reference already exists" }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create_reference(&service, "refs/tags/pkg/v1.2.0")
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::ReferenceExists(ref name) if name == "refs/tags/pkg/v1.2.0"),
            "{failure:?}"
        );
        assert_eq!(
            failure.to_string(),
            "A reference named refs/tags/pkg/v1.2.0 already exists"
        );
    }

    #[tokio::test]
    async fn another_reference_github_will_not_create_is_refused_with_its_words() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            GIT_REFERENCES.to_string(),
            422,
            json!({ "message": "Object does not exist" }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create_reference(&service, "refs/tags/pkg/v1.2.0")
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Refused { status: 422, ref message } if message == "Object does not exist"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_reference_that_is_not_a_full_name_git_accepts_is_never_sent() {
        for reference in [
            "",
            "v1.2.0",
            "tags/v1.2.0",
            "refs/v1.2.0",
            "refs/tags/",
            "refs/tags/v1..2",
            "refs//tags/v1",
            "refs/tags/v1.lock",
            "refs/tags/v 1",
            "refs/tags/-v1",
            "refs/tags/HEAD",
        ] {
            let server = MockServer::start().await;
            untouched(&server).await;
            let service = stand_in(&server).await;

            let failure = create_reference(&service, reference).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::Reference(ref refused)) if refused == reference),
                "{reference:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_reference_is_created_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            format!("/api/v3{GIT_REFERENCES}"),
            201,
            json!({ "ref": "refs/tags/pkg/v1.2.0" }),
        )
        .await;
        let service = enterprise(&server).await;

        create_reference(&service, "refs/tags/pkg/v1.2.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_reference_outside_tags_is_held_only_to_check_ref_format() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(GIT_REFERENCES))
            .and(body_json(json!({
                "ref": "refs/heads/-draft",
                "sha": commit('f').as_str(),
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        create_reference(&service, "refs/heads/-draft")
            .await
            .unwrap();
    }

    async fn create_tag_reference(
        service: &PullRequestService,
        tag: &str,
    ) -> PullRequestResult<()> {
        service
            .create_tag_reference(&project(service), &token(), tag, &object('f'))
            .await
    }

    #[tokio::test]
    async fn a_tag_is_published_under_refs_tags() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            GIT_REFERENCES.to_string(),
            201,
            json!({ "ref": "refs/tags/pkg/v1.2.0" }),
        )
        .await;
        let service = stand_in(&server).await;

        create_tag_reference(&service, "pkg/v1.2.0").await.unwrap();
    }

    #[tokio::test]
    async fn a_tag_already_published_is_reported_by_its_full_reference() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            GIT_REFERENCES.to_string(),
            422,
            json!({ "message": "Reference already exists" }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create_tag_reference(&service, "pkg/v1.2.0")
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::ReferenceExists(ref name) if name == "refs/tags/pkg/v1.2.0"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_tag_git_refuses_is_never_published() {
        for tag in ["", "-v1", "HEAD", "v1..2", "pkg//v1", "v1/"] {
            let server = MockServer::start().await;
            untouched(&server).await;
            let service = stand_in(&server).await;

            let failure = create_tag_reference(&service, tag).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::Tag(ref refused)) if refused == tag),
                "{tag:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tag_is_published_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        reference_creation(
            &server,
            format!("/api/v3{GIT_REFERENCES}"),
            201,
            json!({ "ref": "refs/tags/pkg/v1.2.0" }),
        )
        .await;
        let service = enterprise(&server).await;

        create_tag_reference(&service, "pkg/v1.2.0").await.unwrap();
    }

    #[tokio::test]
    async fn a_tag_write_without_a_token_is_never_sent_under_anonymous_reads() {
        let server = MockServer::start().await;
        untouched(&server).await;
        let service = stand_in(&server).await.with_anonymous_reads();
        let repository = project(&service);
        let empty = SecretValue::new("");

        let failures = [
            service
                .create_tag(&repository, &empty, &tagging())
                .await
                .unwrap_err(),
            service
                .create_reference(&repository, &empty, "refs/tags/v1.2.0", &object('f'))
                .await
                .unwrap_err(),
            service
                .create_tag_reference(&repository, &empty, "v1.2.0", &object('f'))
                .await
                .unwrap_err(),
        ];

        for failure in failures {
            assert!(
                matches!(failure, PullRequestError::AuthenticationRequired),
                "{failure:?}"
            );
        }
    }
}
