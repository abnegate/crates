use super::*;
use crate::parse_error::ParseError;
use crate::pull_request::Page;
use crate::pull_request::Release;
use crate::pull_request::github_release::GitHubRelease;

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
    /// The tag is held to the rules a [`BranchName`] is, which are git's own
    /// for a ref name, so `pkg/v1.2.3` and `v1.0.0+build` are tags, and it
    /// travels as one path segment, so a `/` in it is sent as `%2F`. A tag
    /// that breaks those rules, such as one that is empty, holds `..` or a
    /// control character, is [`PullRequestError::Parse`] with
    /// [`ParseError::Tag`], and nothing is sent. A tag with no release
    /// published under it is [`PullRequestError::NotFound`].
    pub async fn fetch_release(
        &self,
        repository: &Repository,
        token: &SecretValue,
        tag: &str,
    ) -> PullRequestResult<Release> {
        BranchName::parse(tag).map_err(|_| ParseError::Tag(tag.to_string()))?;

        let release: GitHubRelease = self
            .get(
                self.repository_endpoint(repository, &["releases", "tags", tag])?,
                token,
            )
            .await?;
        Ok(Release::from(release))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
