use super::*;
use crate::pull_request::GeneratedNotes;
use crate::pull_request::NotesRequest;
use crate::pull_request::Page;
use crate::pull_request::Release;
use crate::pull_request::ReleaseRequest;
use crate::pull_request::github_generated_notes::GitHubGeneratedNotes;
use crate::pull_request::github_notes_request::GitHubNotesRequest;
use crate::pull_request::github_release::GitHubRelease;
use crate::pull_request::github_release_request::GitHubReleaseRequest;
use crate::pull_request::github_release_update::GitHubReleaseUpdate;
use crate::tag_name::TagName;

impl PullRequestService {
    /// Page `page` of `repository`'s releases, most recently published first,
    /// at `per_page` rows a page up to GitHub's hundred.
    ///
    /// Drafts are listed only to a token that can push to the repository. A
    /// row that leaves out anything GitHub sends on every release, such as its
    /// identifier, tag, link or whether it is a draft, is
    /// [`PullRequestError::GitHubApi`], never a page without it.
    pub async fn list_releases(
        &self,
        repository: &Repository,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Release>> {
        let asked = asked(per_page);
        let url = self.repository_endpoint(repository, &["releases"])?;
        let rows: Vec<GitHubRelease> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let releases = rows.into_iter().map(Release::from).collect();
        Ok(Page::after(releases, page, read, asked))
    }

    /// The release published under `tag`.
    ///
    /// The tag is held to git's own rules for a tag name, so `pkg/v1.2.3`,
    /// `v1.0.0+build` and `+v1` are tags, and it travels as one path segment,
    /// so a `/` in it is sent as `%2F`. A tag git refuses, such as one that
    /// is empty, holds `..` or an ASCII control character, or is led by `-`,
    /// is [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent. A
    /// tag with no release published under it is
    /// [`PullRequestError::NotFound`].
    pub async fn fetch_release(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tag: &str,
    ) -> PullRequestResult<Release> {
        let tag = TagName::parse(tag)?;

        let release: GitHubRelease = self
            .get(
                self.repository_endpoint(repository, &["releases", "tags", tag.as_str()])?,
                token,
            )
            .await?;
        Ok(Release::from(release))
    }

    /// The release GitHub identifies by `id`, as [`Release::id`] names it,
    /// draft or published.
    ///
    /// A draft is found only by a token that can push to the repository. An
    /// identifier no release has is [`PullRequestError::NotFound`].
    pub async fn fetch_release_by_id(
        &self,
        repository: &Repository,
        token: &SecretValue,
        id: u64,
    ) -> PullRequestResult<Release> {
        let id = id.to_string();
        let release: GitHubRelease = self
            .get(
                self.repository_endpoint(repository, &["releases", &id])?,
                token,
            )
            .await?;
        Ok(Release::from(release))
    }

    /// Release notes GitHub writes for a release under the tag `request`
    /// names, from the pull requests merged since the previous release.
    ///
    /// Nothing is created: the tag need not exist yet, and GitHub reads
    /// [`NotesRequest::target`] for where it would be created from. The tag
    /// and any previous tag are held to git's own rules for a tag name, and
    /// one git refuses is [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent.
    pub async fn generate_release_notes(
        &self,
        repository: &Repository,
        token: &SecretValue,
        request: &NotesRequest,
    ) -> PullRequestResult<GeneratedNotes> {
        let tag = TagName::parse(&request.tag)?;
        let previous = request
            .previous_tag
            .as_deref()
            .map(TagName::parse)
            .transpose()?;
        let url = self.repository_endpoint(repository, &["releases", "generate-notes"])?;
        let body = GitHubNotesRequest {
            tag_name: tag.as_str(),
            target_commitish: &request.target,
            previous_tag_name: previous.as_ref().map(TagName::as_str),
        };

        let notes: GitHubGeneratedNotes = self
            .exchange(self.request(Method::POST, url, token, ACCEPT)?.json(&body))
            .await?;
        Ok(GeneratedNotes::from(notes))
    }

    /// Create the release `request` describes, and the release GitHub made.
    ///
    /// GitHub creates the tag from [`ReleaseRequest::target`], or from the
    /// default branch, when it does not exist yet. The tag is held to git's
    /// own rules for a tag name, and one git refuses is
    /// [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`](crate::ParseError::Tag), and nothing is sent. A tag
    /// that already has a release is [`PullRequestError::Refused`] with
    /// GitHub's 422.
    pub async fn create_release(
        &self,
        repository: &Repository,
        token: &SecretValue,
        request: &ReleaseRequest,
    ) -> PullRequestResult<Release> {
        TagName::parse(&request.tag)?;
        let url = self.repository_endpoint(repository, &["releases"])?;
        let body = GitHubReleaseRequest::from(request);

        let release: GitHubRelease = self
            .exchange(self.request(Method::POST, url, token, ACCEPT)?.json(&body))
            .await?;
        Ok(Release::from(release))
    }

    /// Publish the draft release GitHub identifies by `id`, and the release
    /// it now is.
    ///
    /// GitHub creates its tag then if it does not exist yet. Publishing a
    /// release that is already published leaves it as it is. An identifier
    /// no release has is [`PullRequestError::NotFound`].
    pub async fn publish_release(
        &self,
        repository: &Repository,
        token: &SecretValue,
        id: u64,
    ) -> PullRequestResult<Release> {
        let id = id.to_string();
        let url = self.repository_endpoint(repository, &["releases", &id])?;

        let release: GitHubRelease = self
            .exchange(
                self.request(Method::PATCH, url, token, ACCEPT)?
                    .json(&GitHubReleaseUpdate { draft: false }),
            )
            .await?;
        Ok(Release::from(release))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
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

    const RELEASES: &str = "/repos/acme/project/releases";

    const TAG: &str = "v1.4.0";

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    /// What GitHub sends on every release, without which a row is
    /// unreadable.
    const REQUIRED: &[&str] = &["id", "tag_name", "html_url", "draft", "prerelease"];

    fn linked(id: u64, tag: &str) -> Release {
        Release::new(
            id,
            tag,
            format!("https://github.com/acme/project/releases/tag/{tag}"),
        )
    }

    fn published(id: u64) -> Release {
        linked(id, &format!("v1.{id}.0"))
            .with_name(format!("Release {id}"))
            .with_published_at("2026-09-05T00:00:00Z")
    }

    fn described(release: &Release) -> Value {
        json!({
            "id": release.id,
            "tag_name": release.tag,
            "name": release.name,
            "html_url": release.url,
            "published_at": release.published_at,
            "draft": release.draft,
            "prerelease": release.prerelease,
            "body": release.body,
            "target_commitish": release.target,
        })
    }

    async fn listing(server: &MockServer, asked: &str, listed: Vec<Value>) {
        Mock::given(method("GET"))
            .and(path(RELEASES))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(query_is(&[("per_page", asked), ("page", "2")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn list(server: &MockServer, per_page: u8) -> PullRequestResult<Page<Release>> {
        let service = stand_in(server).await;
        service
            .list_releases(&project(&service), &token(), page(2), rows(per_page))
            .await
    }

    async fn fetch(server: &MockServer, tag: &str) -> PullRequestResult<Release> {
        let service = stand_in(server).await;
        service
            .fetch_release(&project(&service), &token(), tag)
            .await
    }

    #[tokio::test]
    async fn a_full_page_of_releases_is_asked_for_and_followed() {
        let server = MockServer::start().await;
        let releases: Vec<Release> = (1..=5).map(published).collect();
        listing(&server, "5", releases.iter().map(described).collect()).await;

        let listed = list(&server, 5).await.unwrap();

        assert_eq!(listed, Page::new(releases, Some(page(3))));
    }

    #[tokio::test]
    async fn a_short_page_of_releases_is_the_last() {
        let server = MockServer::start().await;
        let releases: Vec<Release> = (1..=4).map(published).collect();
        listing(&server, "5", releases.iter().map(described).collect()).await;

        let listed = list(&server, 5).await.unwrap();

        assert_eq!(listed, Page::new(releases, None));
    }

    #[tokio::test]
    async fn no_more_releases_are_asked_for_than_github_sends() {
        let server = MockServer::start().await;
        listing(&server, "100", Vec::new()).await;

        let listed = list(&server, u8::MAX).await.unwrap();

        assert_eq!(listed, Page::new(Vec::new(), None));
    }

    #[tokio::test]
    async fn a_release_with_no_title_or_publication_time_is_read_as_having_none() {
        let server = MockServer::start().await;
        let untitled = linked(91, "v1.5.0");
        let mut omitted = described(&untitled);
        omitted.as_object_mut().unwrap().remove("name");
        omitted.as_object_mut().unwrap().remove("published_at");
        listing(&server, "5", vec![described(&untitled), omitted]).await;

        let listed = list(&server, 5).await.unwrap();

        assert_eq!(listed.items, [untitled.clone(), untitled]);
    }

    #[tokio::test]
    async fn a_release_row_missing_what_github_always_sends_fails_the_whole_page() {
        for required in REQUIRED {
            let server = MockServer::start().await;
            let mut row = described(&published(2));
            row.as_object_mut().unwrap().remove(*required);
            listing(&server, "5", vec![described(&published(1)), row]).await;

            let failure = list(&server, 5).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{required} missing: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_release_is_found_by_its_tag() {
        let server = MockServer::start().await;
        let release = linked(90, TAG)
            .with_name("Version 1.4")
            .with_published_at("2026-09-05T00:00:00Z")
            .with_prerelease(true);
        Mock::given(method("GET"))
            .and(path(format!("{RELEASES}/tags/{TAG}")))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(described(&release)))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(fetch(&server, TAG).await.unwrap(), release);
    }

    #[tokio::test]
    async fn a_draft_is_read_with_no_publication_time() {
        let server = MockServer::start().await;
        let draft = linked(92, "v2.0.0").with_draft(true);
        Mock::given(method("GET"))
            .and(path(format!("{RELEASES}/tags/v2.0.0")))
            .respond_with(ResponseTemplate::new(200).set_body_json(described(&draft)))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(fetch(&server, "v2.0.0").await.unwrap(), draft);
    }

    #[tokio::test]
    async fn an_unknown_tag_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{RELEASES}/tags/v9.9.9")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let failure = fetch(&server, "v9.9.9").await.unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_tag_git_accepts_is_sent_as_one_segment() {
        for (tag, sent) in [
            ("pkg/v1.2.3", "pkg%2Fv1.2.3"),
            ("v1.0.0+build", "v1.0.0+build"),
            ("+v1", "+v1"),
            ("@", "@"),
            ("release_2026-11", "release_2026-11"),
        ] {
            let server = MockServer::start().await;
            let release = linked(93, tag);
            Mock::given(method("GET"))
                .and(path(format!("{RELEASES}/tags/{sent}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(described(&release)))
                .expect(1)
                .mount(&server)
                .await;

            assert_eq!(fetch(&server, tag).await.unwrap(), release, "{tag}");
        }
    }

    #[tokio::test]
    async fn a_tag_a_request_cannot_name_is_refused_unsent() {
        for tag in [
            "",
            "../x",
            ".",
            "..",
            "v1..2",
            "v1?draft=true",
            "v 1",
            "v1\u{7}",
            "v1\n",
            "-v1",
            "pkg//v1",
            "v1/",
            "HEAD",
        ] {
            let server = MockServer::start().await;
            Mock::given(any())
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;

            let failure = fetch(&server, tag).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::Tag(ref refused)) if refused == tag),
                "{tag:?}: {failure:?}"
            );
        }
    }

    const NOTES: &str = "/repos/acme/project/releases/generate-notes";

    /// A mock that answers anything, and that no request may reach.
    async fn untouched(server: &MockServer) {
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(server)
            .await;
    }

    fn noted(id: u64) -> Release {
        published(id)
            .with_body("## What's Changed\n* Basket totals")
            .with_target("main")
    }

    fn missing() -> ResponseTemplate {
        ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" }))
    }

    async fn answering(server: &MockServer, verb: &str, route: String, status: u16, answer: Value) {
        Mock::given(method(verb))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(status).set_body_json(answer))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_release_is_read_with_its_notes_and_target() {
        let server = MockServer::start().await;
        let release = noted(3);
        listing(&server, "5", vec![described(&release)]).await;

        let listed = list(&server, 5).await.unwrap();

        assert_eq!(listed.items, [release]);
    }

    #[tokio::test]
    async fn a_release_is_found_by_its_identifier() {
        let server = MockServer::start().await;
        let draft = noted(91).with_draft(true);
        answering(
            &server,
            "GET",
            format!("{RELEASES}/91"),
            200,
            described(&draft),
        )
        .await;
        let service = stand_in(&server).await;

        let found = service
            .fetch_release_by_id(&project(&service), &token(), 91)
            .await
            .unwrap();

        assert_eq!(found, draft);
    }

    #[tokio::test]
    async fn an_identifier_no_release_has_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{RELEASES}/404")))
            .respond_with(missing())
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_release_by_id(&project(&service), &token(), 404)
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_release_is_found_by_its_identifier_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        answering(
            &server,
            "GET",
            format!("/api/v3{RELEASES}/7"),
            200,
            described(&noted(7)),
        )
        .await;
        let service = enterprise(&server).await;

        let found = service
            .fetch_release_by_id(&project(&service), &token(), 7)
            .await
            .unwrap();

        assert_eq!(found, noted(7));
    }

    async fn notes_for(
        service: &PullRequestService,
        request: &NotesRequest,
    ) -> PullRequestResult<GeneratedNotes> {
        service
            .generate_release_notes(&project(service), &token(), request)
            .await
    }

    async fn writing_notes(server: &MockServer, route: String, sent: Value) {
        Mock::given(method("POST"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(sent))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "v1.2.0",
                "body": "## What's Changed\n* Basket totals",
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn written_notes() -> GeneratedNotes {
        GeneratedNotes::new("v1.2.0", "## What's Changed\n* Basket totals")
    }

    #[tokio::test]
    async fn notes_are_written_since_the_release_github_chooses() {
        let server = MockServer::start().await;
        writing_notes(
            &server,
            NOTES.to_string(),
            json!({ "tag_name": "v1.2.0", "target_commitish": "main" }),
        )
        .await;
        let service = stand_in(&server).await;

        let notes = notes_for(&service, &NotesRequest::new("v1.2.0", "main"))
            .await
            .unwrap();

        assert_eq!(notes, written_notes());
    }

    #[tokio::test]
    async fn notes_are_written_since_the_release_asked_for() {
        let server = MockServer::start().await;
        writing_notes(
            &server,
            NOTES.to_string(),
            json!({
                "tag_name": "pkg/v1.2.0",
                "target_commitish": "main",
                "previous_tag_name": "pkg/v1.1.0",
            }),
        )
        .await;
        let service = stand_in(&server).await;
        let request = NotesRequest::new("pkg/v1.2.0", "main").with_previous_tag("pkg/v1.1.0");

        let notes = notes_for(&service, &request).await.unwrap();

        assert_eq!(notes, written_notes());
    }

    #[tokio::test]
    async fn notes_for_a_tag_git_refuses_are_never_asked_for() {
        for request in [
            NotesRequest::new("v1..2", "main"),
            NotesRequest::new("v1.2.0", "main").with_previous_tag("-v1"),
        ] {
            let server = MockServer::start().await;
            untouched(&server).await;
            let service = stand_in(&server).await;

            let failure = notes_for(&service, &request).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::Tag(_))),
                "{request:?}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn notes_github_will_not_write_are_refused_with_its_words() {
        let server = MockServer::start().await;
        answering(
            &server,
            "POST",
            NOTES.to_string(),
            422,
            json!({ "message": "Validation Failed", "errors": ["target_commitish is invalid"] }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = notes_for(&service, &NotesRequest::new("v1.2.0", "nowhere"))
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Refused { status: 422, ref message } if message == "Validation Failed; target_commitish is invalid"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn notes_are_written_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        writing_notes(
            &server,
            format!("/api/v3{NOTES}"),
            json!({ "tag_name": "v1.2.0", "target_commitish": "main" }),
        )
        .await;
        let service = enterprise(&server).await;

        let notes = notes_for(&service, &NotesRequest::new("v1.2.0", "main"))
            .await
            .unwrap();

        assert_eq!(notes, written_notes());
    }

    async fn create(
        service: &PullRequestService,
        request: &ReleaseRequest,
    ) -> PullRequestResult<Release> {
        service
            .create_release(&project(service), &token(), request)
            .await
    }

    async fn creating(server: &MockServer, route: String, sent: Value, answer: &Release) {
        Mock::given(method("POST"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(sent))
            .respond_with(ResponseTemplate::new(201).set_body_json(described(answer)))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_release_is_created_sending_only_what_was_set() {
        let server = MockServer::start().await;
        let made = linked(95, "v1.2.0").with_target("main");
        creating(
            &server,
            RELEASES.to_string(),
            json!({ "tag_name": "v1.2.0", "draft": false, "prerelease": false }),
            &made,
        )
        .await;
        let service = stand_in(&server).await;

        let created = create(&service, &ReleaseRequest::new("v1.2.0"))
            .await
            .unwrap();

        assert_eq!(created, made);
    }

    #[tokio::test]
    async fn a_draft_release_is_created_with_everything_it_was_given() {
        let server = MockServer::start().await;
        let made = linked(96, "pkg/v1.2.0")
            .with_name("Version 1.2")
            .with_body("Adds basket totals.")
            .with_target("release/1.2")
            .with_draft(true)
            .with_prerelease(true);
        creating(
            &server,
            RELEASES.to_string(),
            json!({
                "tag_name": "pkg/v1.2.0",
                "target_commitish": "release/1.2",
                "name": "Version 1.2",
                "body": "Adds basket totals.",
                "draft": true,
                "prerelease": true,
            }),
            &made,
        )
        .await;
        let service = stand_in(&server).await;
        let request = ReleaseRequest::new("pkg/v1.2.0")
            .with_target("release/1.2")
            .with_name("Version 1.2")
            .with_body("Adds basket totals.")
            .with_draft(true)
            .with_prerelease(true);

        let created = create(&service, &request).await.unwrap();

        assert_eq!(created, made);
    }

    #[tokio::test]
    async fn a_release_for_a_tag_that_already_has_one_is_refused_with_its_words() {
        let server = MockServer::start().await;
        answering(
            &server,
            "POST",
            RELEASES.to_string(),
            422,
            json!({
                "message": "Validation Failed",
                "errors": [{ "resource": "Release", "code": "already_exists", "field": "tag_name" }],
            }),
        )
        .await;
        let service = stand_in(&server).await;

        let failure = create(&service, &ReleaseRequest::new("v1.2.0"))
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Refused { status: 422, .. }),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_release_under_a_tag_git_refuses_is_never_sent() {
        let server = MockServer::start().await;
        untouched(&server).await;
        let service = stand_in(&server).await;

        let failure = create(&service, &ReleaseRequest::new("HEAD"))
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Parse(ParseError::Tag(ref refused)) if refused == "HEAD"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_release_is_created_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        let made = linked(97, "v1.2.0");
        creating(
            &server,
            format!("/api/v3{RELEASES}"),
            json!({ "tag_name": "v1.2.0", "draft": false, "prerelease": false }),
            &made,
        )
        .await;
        let service = enterprise(&server).await;

        let created = create(&service, &ReleaseRequest::new("v1.2.0"))
            .await
            .unwrap();

        assert_eq!(created, made);
    }

    async fn publish(service: &PullRequestService, id: u64) -> PullRequestResult<Release> {
        service
            .publish_release(&project(service), &token(), id)
            .await
    }

    async fn publishing(server: &MockServer, route: String, answer: &Release) {
        Mock::given(method("PATCH"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({ "draft": false })))
            .respond_with(ResponseTemplate::new(200).set_body_json(described(answer)))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_draft_is_published_by_its_identifier() {
        let server = MockServer::start().await;
        let release = noted(98);
        publishing(&server, format!("{RELEASES}/98"), &release).await;
        let service = stand_in(&server).await;

        assert_eq!(publish(&service, 98).await.unwrap(), release);
    }

    #[tokio::test]
    async fn publishing_an_identifier_no_release_has_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("{RELEASES}/404")))
            .respond_with(missing())
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = publish(&service, 404).await.unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_draft_is_published_under_an_enterprise_origin_s_path() {
        let server = MockServer::start().await;
        let release = noted(99);
        publishing(&server, format!("/api/v3{RELEASES}/99"), &release).await;
        let service = enterprise(&server).await;

        assert_eq!(publish(&service, 99).await.unwrap(), release);
    }

    #[tokio::test]
    async fn a_release_write_without_a_token_is_never_sent_under_anonymous_reads() {
        let server = MockServer::start().await;
        untouched(&server).await;
        let service = stand_in(&server).await.with_anonymous_reads();
        let repository = project(&service);
        let empty = SecretValue::new("");

        let failures = [
            service
                .create_release(&repository, &empty, &ReleaseRequest::new("v1.2.0"))
                .await
                .unwrap_err(),
            service
                .publish_release(&repository, &empty, 1)
                .await
                .unwrap_err(),
            service
                .generate_release_notes(&repository, &empty, &NotesRequest::new("v1.2.0", "main"))
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
