//! Pull requests on GitHub, and how each one was received.
//!
//! [`PrService`] opens a pull request when a task completes with code changes,
//! and reads back when it merged, how many rounds of review it took, who
//! approved it, and what the reviewers actually said. It addresses GitHub's own
//! API by default and a GitHub Enterprise origin when one is configured; either
//! way it only answers for repositories on the host that origin serves.

use chrono::DateTime;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

/// What this crate calls itself to the GitHub API.
const USER_AGENT: &str = "abnegate-vcs";

/// GitHub's own REST origin, which answers for repositories on `github.com`.
const GITHUB_API_URL: &str = "https://api.github.com";

/// Rows GitHub returns per page; its maximum for these collections.
const PAGE_SIZE: usize = 100;

/// What a pull request says, for a reviewer who was not in the chat.
///
/// The problem comes first because it is what the reviewer judges the change
/// against, then the run's own report of what it did about it, then the files
/// it touched. A run that reported nothing leaves its section out rather than
/// heading an empty one.
pub struct Description<'a> {
    pub problem: &'a str,
    pub report: Option<&'a str>,
    pub changes: Option<&'a str>,
    pub task: Uuid,
    pub url: Option<&'a str>,
}

impl Description<'_> {
    pub fn render(&self) -> String {
        let mut body = format!("## Problem\n\n{}\n\n", self.problem.trim());

        if let Some(report) = self.report.map(str::trim).filter(|it| !it.is_empty()) {
            body.push_str(&format!("## What changed\n\n{report}\n\n"));
        }

        if let Some(changes) = self.changes.map(str::trim).filter(|it| !it.is_empty()) {
            body.push_str(&format!("## Files\n\n{changes}\n\n"));
        }

        body.push_str("---\n");
        match self.url {
            Some(url) => body.push_str(&format!("Opened from [this task]({url}).\n")),
            None => body.push_str(&format!("Opened from task `{}`.\n", self.task)),
        }
        body
    }
}

/// Pages a paged read will follow before it stops. A pull request with more
/// review activity than this has long since stopped teaching anything new, and
/// an unbounded walk would let one pathological change stall the sync.
const MAXIMUM_PAGES: usize = 10;

/// PR service errors
#[derive(Debug, Error)]
pub enum PrError {
    #[error("GitHub API error: {0}")]
    GitHubApi(String),

    #[error("Repository not configured")]
    NoRepository,

    #[error("Authentication failed")]
    AuthFailed,

    #[error("Branch not found: {0}")]
    BranchNotFound(String),

    #[error("PR already exists for branch: {0}")]
    PrAlreadyExists(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid repository URL: {0}")]
    InvalidRepoUrl(String),
}

pub type PrResult<T> = Result<T, PrError>;

/// GitHub pull request response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubPullRequest {
    pub id: i64,
    pub number: i64,
    pub html_url: String,
    pub state: String,
    pub title: String,
    pub body: Option<String>,
    pub head: GitHubBranch,
    pub base: GitHubBranch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubBranch {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

/// Request to create a pull request
#[derive(Debug, Clone, Serialize)]
struct CreatePrRequest {
    title: String,
    body: String,
    head: String,
    base: String,
    draft: bool,
}

/// Created PR result
#[derive(Debug, Clone)]
pub struct CreatedPr {
    pub url: String,
    pub number: i64,
    pub state: String,
}

/// Where a pull request lives, recovered from the URL a run recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReference {
    pub owner: String,
    pub repository: String,
    pub number: i64,
}

/// The verdict a reviewer submitted with a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
    Unknown,
}

impl ReviewState {
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_uppercase().as_str() {
            "APPROVED" => ReviewState::Approved,
            "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
            "COMMENTED" => ReviewState::Commented,
            "DISMISSED" => ReviewState::Dismissed,
            "PENDING" => ReviewState::Pending,
            _ => ReviewState::Unknown,
        }
    }

    /// Whether this verdict replaces the reviewer's previous one.
    fn settles(self) -> bool {
        matches!(
            self,
            ReviewState::Approved | ReviewState::ChangesRequested | ReviewState::Dismissed
        )
    }
}

/// One submitted review, reduced to the two things the tally needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedReview {
    pub state: ReviewState,
    pub reviewer: Option<String>,
}

/// What the submitted reviews add up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReviewTally {
    pub cycles: u32,
    pub approvals: u32,
}

/// A round of review is a reviewer sending the change back, so every
/// `CHANGES_REQUESTED` costs a cycle. Approvals count distinct people whose
/// latest verdict was an approval: approving twice is still one approval, and a
/// reviewer who later requests changes has withdrawn theirs.
pub fn tally(reviews: &[SubmittedReview]) -> ReviewTally {
    let mut cycles = 0u32;
    let mut latest: Vec<(String, ReviewState)> = Vec::new();
    let mut unattributed = 0u32;

    for review in reviews {
        if review.state == ReviewState::ChangesRequested {
            cycles = cycles.saturating_add(1);
        }

        let reviewer = review
            .reviewer
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());

        match reviewer {
            Some(name) if review.state.settles() => {
                match latest.iter_mut().find(|(known, _)| known == name) {
                    Some(entry) => entry.1 = review.state,
                    None => latest.push((name.to_string(), review.state)),
                }
            }
            None if review.state == ReviewState::Approved => {
                unattributed = unattributed.saturating_add(1);
            }
            _ => {}
        }
    }

    let named = latest
        .iter()
        .filter(|(_, state)| *state == ReviewState::Approved)
        .count();

    ReviewTally {
        cycles,
        approvals: u32::try_from(named)
            .unwrap_or(u32::MAX)
            .saturating_add(unattributed),
    }
}

/// Whole minutes between two RFC 3339 instants, or `None` if either is unreadable.
pub fn minutes_between(opened: &str, merged: &str) -> Option<i64> {
    let opened = DateTime::parse_from_rfc3339(opened).ok()?;
    let merged = DateTime::parse_from_rfc3339(merged).ok()?;
    Some((merged.with_timezone(&Utc) - opened.with_timezone(&Utc)).num_minutes())
}

/// How a change was received once the agent handed it over.
///
/// Timestamps stay RFC 3339 and tallies stay plain counts, because this is read
/// back out of a run's artifacts by the learning loop rather than by a person.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequestReception {
    pub opened_at: Option<String>,
    pub merged_at: Option<String>,
    pub minutes_to_merge: Option<i64>,
    pub review_cycles: u32,
    pub approvals: u32,
    pub state: Option<String>,
    pub comments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubUser {
    #[serde(default)]
    login: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubReview {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<GitHubUser>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubComment {
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubPullRequestDetail {
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    mergeable: Option<bool>,
}

/// Whether a pull request's branch still merges with its base.
///
/// GitHub computes this in the background and answers `null` until it has,
/// which is a third answer rather than a missing one: a branch nobody has
/// checked yet is not a branch known to conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeability {
    Clean,
    Conflicted,
    Unknown,
}

impl Mergeability {
    fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            Some(true) => Mergeability::Clean,
            Some(false) => Mergeability::Conflicted,
            None => Mergeability::Unknown,
        }
    }

    pub fn conflicted(self) -> bool {
        self == Mergeability::Conflicted
    }
}

/// PR service for creating pull requests
#[derive(Debug, Clone)]
pub struct PrService {
    client: Client,
    origin: Origin,
}

/// The API origin this service addresses, and the repository host it answers for.
///
/// The two are decided together because every request is
/// `{origin}/repos/{owner}/{repository}/...`: the owner and repository come from
/// a repository URL, and the origin they are interpolated into has to be the one
/// that answers for that URL's host. Deciding them apart is what let a
/// `github.com` repository be accepted while its request -- and the token sent
/// with it -- went to a configured Enterprise origin.
#[derive(Debug, Clone)]
struct Origin {
    url: String,
    host: String,
}

impl Origin {
    /// The origin an operator configured. It answers for its own host, less the
    /// `api.` label carried by GitHub's own API host and by a subdomain-isolated
    /// Enterprise one; an Enterprise install without subdomain isolation answers
    /// for itself under a `/api/v3` path.
    fn configured(url: String) -> Self {
        let authority = url
            .split_once("://")
            .map_or(url.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default();
        let host = host_of(authority).to_ascii_lowercase();
        let host = host.strip_prefix("api.").unwrap_or(&host).to_string();
        Self { url, host }
    }

    fn standing_in_for(host: &str, url: String) -> Self {
        Self {
            url,
            host: host.to_ascii_lowercase(),
        }
    }

    /// Whether a repository on `host` is one this origin can be asked about.
    fn answers_for(&self, host: &str) -> bool {
        !self.host.is_empty() && host.eq_ignore_ascii_case(&self.host)
    }
}

impl Default for PrService {
    fn default() -> Self {
        Self::new()
    }
}

/// A single path segment that is safe to interpolate into a request URL.
/// The host of an authority, without userinfo or port.
fn host_of(authority: &str) -> &str {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    match host.strip_prefix('[') {
        Some(tail) => tail.split_once(']').map_or(host, |(inside, _)| inside),
        None => host.split_once(':').map_or(host, |(host, _)| host),
    }
}

/// Schemes a repository address may carry.
///
/// The scp-like `git@host:owner/repo` has none and is read on its own terms.
const SCHEMES: [&str; 2] = ["https", "ssh"];

fn named(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

impl PrService {
    /// Address GitHub's own API.
    pub fn new() -> Self {
        Self::configured(GITHUB_API_URL.to_string())
    }

    /// Address the origin an operator configured, for the repositories it
    /// answers for.
    pub fn configured(url: String) -> Self {
        Self {
            client: Client::new(),
            origin: Origin::configured(url),
        }
    }

    /// Address `url` as a stand-in for repositories on `host`.
    ///
    /// No operator setting produces one: a configured origin has to answer for
    /// a host it can be reached at. Publication tests use this to drive the
    /// real request path against a mock server.
    pub fn standing_in_for(host: &str, url: String) -> Self {
        Self {
            client: Client::new(),
            origin: Origin::standing_in_for(host, url),
        }
    }

    /// Parse owner and repo from a repository URL.
    ///
    /// The host has to be the one this service's origin answers for, so that a
    /// GitHub Enterprise install parses its own repositories and nothing else.
    /// Matching `https://github.com/` literally was what made `GITHUB_API_URL`
    /// insufficient on its own to reach Enterprise; matching nothing at all
    /// would accept a repository this service cannot open a request against;
    /// and admitting `github.com` alongside the configured origin sent a
    /// github.com repository's token to whichever install was configured.
    ///
    /// Supports, for the host it answers for:
    ///
    /// - `https://host/owner/repo`, with or without `.git`
    /// - `git@host:owner/repo.git`
    /// - `ssh://git@host/owner/repo.git`
    ///
    /// A scheme this does not speak is refused rather than discarded: reading
    /// the owner and repo out of an `ftp://` or `http://` address treats it as
    /// a repository this service publishes to, which is not what it is.
    pub fn parse_github_url(&self, url: &str) -> PrResult<(String, String)> {
        let invalid = || PrError::InvalidRepoUrl(url.to_string());
        let url = url.trim();

        // `git@host:owner/repo` is not a URL, so it is split on the colon
        // rather than parsed. The scp-like form has no scheme to strip.
        let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
            if !SCHEMES
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
            {
                return Err(invalid());
            }
            rest.split_once('/').ok_or_else(invalid)?
        } else if let Some((authority, path)) = url.split_once(':') {
            (authority, path)
        } else {
            return Err(invalid());
        };

        if !self.origin.answers_for(host_of(authority)) {
            return Err(invalid());
        }

        let path = path.trim_matches('/').trim_end_matches(".git");
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let owner = segments.next().ok_or_else(invalid)?;
        let repository = segments.next().ok_or_else(invalid)?;
        // The pair is interpolated into `{base}/repos/{owner}/{repo}/...`, so a
        // third segment or a traversal component would reach a different
        // endpoint than the caller asked for.
        if segments.next().is_some() || !named(owner) || !named(repository) {
            return Err(invalid());
        }

        Ok((owner.to_string(), repository.to_string()))
    }

    /// Recover where a pull request lives from the URL a run recorded.
    ///
    /// Reads `https://host/owner/repo/pull/123`, with or without a trailing
    /// segment such as `/files` that a person's copied link often carries. The
    /// host is held to the same origin as a repository URL, because the pull
    /// request is read back from `{origin}/repos/{owner}/{repo}/pulls/{number}`
    /// -- a recorded github.com link would otherwise be read from, and
    /// authenticated against, whichever install happened to be configured.
    pub fn pull_request(&self, url: &str) -> PrResult<PullRequestReference> {
        let invalid = || PrError::InvalidRepoUrl(url.to_string());
        let (scheme, rest) = url.trim().split_once("://").ok_or_else(invalid)?;
        let (authority, path) = rest.split_once('/').ok_or_else(invalid)?;

        if !matches!(scheme, "http" | "https") || !self.origin.answers_for(host_of(authority)) {
            return Err(invalid());
        }

        let mut segments = path.split('/');
        let owner = segments.next().unwrap_or_default();
        let repository = segments.next().unwrap_or_default().trim_end_matches(".git");
        let marker = segments.next().unwrap_or_default();
        let number: i64 = segments
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| invalid())?;

        if marker != "pull" || number < 1 || !named(owner) || !named(repository) {
            return Err(invalid());
        }

        Ok(PullRequestReference {
            owner: owner.to_string(),
            repository: repository.to_string(),
            number,
        })
    }

    /// Create a pull request on GitHub
    pub async fn create_pr(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        head_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> PrResult<CreatedPr> {
        let url = format!("{}/repos/{}/{}/pulls", self.origin.url, owner, repo);

        let request = CreatePrRequest {
            title: title.to_string(),
            body: body.to_string(),
            head: head_branch.to_string(),
            base: base_branch.to_string(),
            draft,
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .json(&request)
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            let pr: GitHubPullRequest = response.json().await?;
            return Ok(CreatedPr {
                url: pr.html_url,
                number: pr.number,
                state: pr.state,
            });
        }

        // Handle error responses
        let error_text = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PrError::AuthFailed);
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PrError::BranchNotFound(head_branch.to_string()));
        }

        if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
            && error_text.contains("A pull request already exists")
        {
            return Err(PrError::PrAlreadyExists(head_branch.to_string()));
        }

        Err(PrError::GitHubApi(format!(
            "GitHub API returned {}: {}",
            status, error_text
        )))
    }

    /// Get the default branch for a repository
    pub async fn get_default_branch(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> PrResult<String> {
        let url = format!("{}/repos/{}/{}", self.origin.url, owner, repo);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "Failed to get repo info: {} {}",
                status, error_text
            )));
        }

        #[derive(Deserialize)]
        struct RepoInfo {
            default_branch: String,
        }

        let repo_info: RepoInfo = response.json().await?;
        Ok(repo_info.default_branch)
    }

    /// Check if a PR already exists for a branch
    pub async fn pr_exists_for_branch(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        head_branch: &str,
    ) -> PrResult<Option<String>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?head={}:{}&state=open",
            self.origin.url, owner, repo, owner, head_branch
        );

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "Failed to check PRs: {} {}",
                status, error_text
            )));
        }

        let prs: Vec<GitHubPullRequest> = response.json().await?;

        if let Some(pr) = prs.first() {
            Ok(Some(pr.html_url.clone()))
        } else {
            Ok(None)
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str, token: &str) -> PrResult<T> {
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PrError::AuthFailed);
        }
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "GitHub API returned {}: {}",
                status, error_text
            )));
        }

        Ok(response.json().await?)
    }

    async fn get_all<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
    ) -> PrResult<Vec<T>> {
        let mut collected: Vec<T> = Vec::new();

        for page in 1..=MAXIMUM_PAGES {
            let url = format!(
                "{}/{}?per_page={}&page={}",
                self.origin.url, path, PAGE_SIZE, page
            );
            let batch: Vec<T> = self.get(&url, token).await?;
            let complete = batch.len() < PAGE_SIZE;
            collected.extend(batch);
            if complete {
                break;
            }
        }

        Ok(collected)
    }

    /// Ask GitHub whether a pull request's branch still merges with its base.
    ///
    /// One read, so a caller can skip the expensive work of reproducing a merge
    /// for the overwhelming majority of branches that do not conflict.
    pub async fn fetch_mergeability(
        &self,
        reference: &PullRequestReference,
        token: &str,
    ) -> PrResult<Mergeability> {
        let detail: GitHubPullRequestDetail = self
            .get(
                &format!(
                    "{}/repos/{}/{}/pulls/{}",
                    self.origin.url, reference.owner, reference.repository, reference.number
                ),
                token,
            )
            .await?;

        Ok(Mergeability::from_flag(detail.mergeable))
    }

    /// Read back how a pull request was received.
    ///
    /// Three reads: the pull request itself for the opening and merge times, its
    /// reviews for the cycle and approval tallies, and its inline review comments.
    /// Review bodies count as comments too, because a reviewer who requests changes
    /// with a paragraph of reasoning is saying the same thing as one who writes it
    /// on a line.
    pub async fn fetch_reception(
        &self,
        reference: &PullRequestReference,
        token: &str,
    ) -> PrResult<PullRequestReception> {
        let scope = format!("repos/{}/{}", reference.owner, reference.repository);

        let detail: GitHubPullRequestDetail = self
            .get(
                &format!("{}/{}/pulls/{}", self.origin.url, scope, reference.number),
                token,
            )
            .await?;

        let reviews: Vec<GitHubReview> = self
            .get_all(
                &format!("{}/pulls/{}/reviews", scope, reference.number),
                token,
            )
            .await?;

        let inline: Vec<GitHubComment> = self
            .get_all(
                &format!("{}/pulls/{}/comments", scope, reference.number),
                token,
            )
            .await?;

        let submitted: Vec<SubmittedReview> = reviews
            .iter()
            .map(|review| SubmittedReview {
                state: ReviewState::parse(review.state.as_deref().unwrap_or_default()),
                reviewer: review.user.as_ref().and_then(|user| user.login.clone()),
            })
            .collect();
        let tallied = tally(&submitted);

        let comments: Vec<String> = reviews
            .iter()
            .filter_map(|review| review.body.as_deref())
            .chain(inline.iter().filter_map(|comment| comment.body.as_deref()))
            .map(|body| body.trim().to_string())
            .filter(|body| !body.is_empty())
            .collect();

        let opened_at = detail.created_at.filter(|value| !value.is_empty());
        let merged_at = detail.merged_at.filter(|value| !value.is_empty());
        let minutes_to_merge = match (&opened_at, &merged_at) {
            (Some(opened), Some(merged)) => minutes_between(opened, merged),
            _ => None,
        };

        Ok(PullRequestReception {
            opened_at,
            merged_at,
            minutes_to_merge,
            review_cycles: tallied.cycles,
            approvals: tallied.approvals,
            state: detail.state.filter(|value| !value.is_empty()),
            comments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_https_url() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("https://github.com/acme/project")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    /// Matching `https://github.com/` meant an Enterprise repository was
    /// refused as invalid, so `GITHUB_API_URL` alone could not reach one.
    #[test]
    fn an_enterprise_repository_parses_like_a_github_one() {
        let service = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://github.example.com/acme/project",
            "https://github.example.com/acme/project.git",
            "ssh://git@github.example.com/acme/project.git",
            "git@github.example.com:acme/project.git",
            "  https://github.example.com/acme/project/  ",
        ] {
            assert_eq!(
                service.parse_github_url(url).expect(url),
                ("acme".to_string(), "project".to_string()),
                "{url}"
            );
        }
    }

    /// A subdomain-isolated Enterprise install serves its API from `api.` on
    /// the host its repositories live on, exactly as api.github.com does for
    /// github.com.
    #[test]
    fn an_api_subdomain_answers_for_the_host_beneath_it() {
        for origin in [
            "https://api.github.example.com",
            "https://github.example.com/api/v3",
        ] {
            assert_eq!(
                PrService::configured(origin.to_string())
                    .parse_github_url("https://github.example.com/acme/project")
                    .expect(origin),
                ("acme".to_string(), "project".to_string()),
                "{origin}"
            );
        }
    }

    /// The repository's host picks the origin its owner and repository are
    /// interpolated into, and only the configured origin's own host has one. A
    /// github.com repository accepted here would have had its request -- and
    /// its access token -- sent to the Enterprise install instead.
    #[test]
    fn a_github_repository_has_no_origin_while_enterprise_is_configured() {
        let enterprise = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "https://github.com/acme/project/pull/7",
        ] {
            assert!(
                enterprise.parse_github_url(url).is_err(),
                "{url} must not be addressed at an origin that does not answer for github.com"
            );
        }
        assert!(
            enterprise
                .pull_request("https://github.com/acme/project/pull/7")
                .is_err(),
            "a recorded github.com pull request must not be read from the Enterprise origin"
        );
    }

    /// The public path is what almost every deployment runs, so it has to stay
    /// exactly as it was: GitHub's own origin answers for github.com.
    #[test]
    fn githubs_own_origin_answers_for_github_repositories() {
        for service in [
            PrService::new(),
            PrService::configured(GITHUB_API_URL.to_string()),
        ] {
            assert_eq!(
                service
                    .parse_github_url("https://github.com/acme/project")
                    .expect("github.com is what api.github.com answers for"),
                ("acme".to_string(), "project".to_string())
            );
            assert_eq!(
                service
                    .pull_request("https://github.com/acme/project/pull/7")
                    .expect("a github.com pull request is read from api.github.com")
                    .number,
                7
            );
        }
    }

    /// The endpoint the pair is interpolated into belongs to one host, so a
    /// repository somewhere else is not this service's to address -- whatever
    /// its path happens to look like.
    #[test]
    fn a_repository_on_another_host_is_refused() {
        let service = PrService::new();
        for url in [
            "https://gitlab.com/acme/project",
            "https://bitbucket.org/acme/project.git",
            "git@gitlab.com:acme/project.git",
            "https://github.com.attacker.test/acme/project",
            "https://notgithub.com/acme/project",
            "https://github.example.com/acme/project",
        ] {
            assert!(
                service.parse_github_url(url).is_err(),
                "{url} was accepted for api.github.com"
            );
        }

        let enterprise = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://gitlab.com/acme/project",
            "https://github.example.com.attacker.test/acme/project",
        ] {
            assert!(
                enterprise.parse_github_url(url).is_err(),
                "a configured enterprise origin does not admit {url}"
            );
        }
    }

    /// The pair is interpolated into `{base}/repos/{owner}/{repo}/pulls`, so
    /// anything that could reach a different endpoint has to be refused. The
    /// old parser split into two and kept every remaining slash in `repo`.
    #[test]
    fn a_path_that_could_reach_another_endpoint_is_refused() {
        let service = PrService::new();
        for url in [
            "https://github.com/acme/project/extra",
            "https://github.com/acme/../admin",
            "https://github.com/../acme",
            "https://github.com/acme",
            "https://github.com/",
            "https://github.com/acme/pro ject",
            "not-a-url",
            "",
        ] {
            assert!(
                service.parse_github_url(url).is_err(),
                "{url:?} must be refused"
            );
        }
    }

    #[test]
    fn test_parse_github_https_url_with_git() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("https://github.com/acme/project.git")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_github_ssh_url() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("git@github.com:acme/project.git")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_invalid_url() {
        let service = PrService::new();
        let result = service.parse_github_url("not-a-github-url");
        assert!(result.is_err());
    }

    fn task() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    /// The reviewer was not in the chat, so what the change was for comes
    /// before what it did about it, and the run's own report is the account of
    /// the second.
    #[test]
    fn a_description_leads_with_the_problem_and_then_the_run_s_own_report() {
        let body = Description {
            problem: "The login form accepts an invalid email.",
            report: Some("Validated the address before submit. Added a regression test."),
            changes: Some("- `auth.rs`"),
            task: task(),
            url: Some("https://tasks.example.com/tasks/123"),
        }
        .render();

        let problem = body.find("## Problem").expect("{body}");
        let changed = body.find("## What changed").expect("{body}");
        let files = body.find("## Files").expect("{body}");

        assert!(problem < changed && changed < files, "{body}");
        assert!(body.contains("accepts an invalid email"), "{body}");
        assert!(body.contains("Added a regression test."), "{body}");
        assert!(body.contains("`auth.rs`"), "{body}");
        assert!(
            body.contains("[this task](https://tasks.example.com/tasks/123)"),
            "{body}"
        );
    }

    /// A heading over nothing reads as a section the reviewer has missed.
    #[test]
    fn a_run_that_reported_nothing_heads_no_empty_section() {
        for report in [None, Some(""), Some("   \n ")] {
            let body = Description {
                problem: "Something was wrong.",
                report,
                changes: None,
                task: task(),
                url: None,
            }
            .render();

            assert!(!body.contains("## What changed"), "{body}");
            assert!(!body.contains("## Files"), "{body}");
            assert!(body.contains("## Problem"), "{body}");
        }
    }

    /// Without a console to link to, the id is what takes a reviewer back to
    /// the run that opened this.
    #[test]
    fn a_description_without_a_console_link_names_the_task_it_came_from() {
        let body = Description {
            problem: "Something was wrong.",
            report: None,
            changes: None,
            task: task(),
            url: None,
        }
        .render();

        assert!(body.contains(&task().to_string()), "{body}");
        assert!(!body.contains("this task]("), "{body}");
    }

    fn review(state: &str, reviewer: Option<&str>) -> SubmittedReview {
        SubmittedReview {
            state: ReviewState::parse(state),
            reviewer: reviewer.map(str::to_string),
        }
    }

    #[test]
    fn a_pull_request_url_yields_its_owner_repository_and_number() {
        let reference = PrService::new()
            .pull_request("https://github.com/acme/project/pull/42")
            .expect("a plain pull request URL must parse");
        assert_eq!(reference.owner, "acme");
        assert_eq!(reference.repository, "project");
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn a_pull_request_url_parses_past_a_trailing_tab_segment() {
        let reference = PrService::new()
            .pull_request("https://github.com/acme/project/pull/42/files")
            .unwrap();
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn a_url_that_is_not_a_pull_request_is_refused() {
        let service = PrService::new();
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project/issues/42",
            "https://github.com/acme/project/pull/zero",
            "https://github.com/acme/project/pull/0",
            "https://github.com/../project/pull/42",
            "https://example.test/acme/project/pull/42",
            "git@github.com:acme/project/pull/42",
            "",
        ] {
            assert!(
                service.pull_request(url).is_err(),
                "{url} is not a pull request and must not parse as one"
            );
        }
    }

    #[test]
    fn every_request_for_changes_costs_a_review_cycle() {
        let tallied = tally(&[
            review("CHANGES_REQUESTED", Some("ada")),
            review("COMMENTED", Some("grace")),
            review("CHANGES_REQUESTED", Some("grace")),
        ]);
        assert_eq!(tallied.cycles, 2);
    }

    #[test]
    fn a_reviewer_who_approves_twice_is_still_one_approval() {
        let tallied = tally(&[
            review("APPROVED", Some("ada")),
            review("APPROVED", Some("ada")),
        ]);
        assert_eq!(tallied.approvals, 1);
        assert_eq!(tallied.cycles, 0);
    }

    #[test]
    fn an_approval_withdrawn_by_a_later_request_for_changes_stops_counting() {
        let tallied = tally(&[
            review("APPROVED", Some("ada")),
            review("CHANGES_REQUESTED", Some("ada")),
        ]);
        assert_eq!(
            tallied.approvals, 0,
            "a reviewer who came back asking for changes has withdrawn their approval"
        );
        assert_eq!(tallied.cycles, 1);
    }

    #[test]
    fn a_comment_only_review_is_neither_a_cycle_nor_an_approval() {
        let tallied = tally(&[review("COMMENTED", Some("ada")), review("PENDING", None)]);
        assert_eq!(tallied, ReviewTally::default());
    }

    #[test]
    fn an_unknown_review_state_is_counted_as_neither() {
        let tallied = tally(&[review("SOMETHING_NEW", Some("ada"))]);
        assert_eq!(tallied, ReviewTally::default());
        assert_eq!(ReviewState::parse("something_new"), ReviewState::Unknown);
    }

    #[test]
    fn merge_minutes_are_whole_and_reject_unreadable_timestamps() {
        assert_eq!(
            minutes_between("2026-09-04T09:00:00Z", "2026-09-04T11:30:00Z"),
            Some(150)
        );
        assert_eq!(
            minutes_between("2026-09-04T09:00:00+02:00", "2026-09-04T09:00:00Z"),
            Some(120),
            "offsets must be normalised before subtracting"
        );
        assert_eq!(minutes_between("soon", "2026-09-04T11:30:00Z"), None);
        assert_eq!(minutes_between("2026-09-04T09:00:00Z", ""), None);
    }

    #[tokio::test]
    async fn reception_reads_the_merge_time_cycles_approvals_and_comments() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "created_at": "2026-09-04T09:00:00Z",
                "merged_at": "2026-09-04T10:00:00Z",
                "state": "closed",
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "state": "CHANGES_REQUESTED", "body": "needs a regression test", "user": { "login": "ada" } },
                { "state": "APPROVED", "body": "", "user": { "login": "ada" } },
                { "state": "APPROVED", "body": null, "user": { "login": "grace" } },
            ])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "body": "rename this variable" },
                { "body": "   " },
            ])))
            .mount(&server)
            .await;

        let service = PrService::standing_in_for("github.com", server.uri());
        let reception = service
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .expect("a reachable pull request must yield its reception");

        assert_eq!(reception.opened_at.as_deref(), Some("2026-09-04T09:00:00Z"));
        assert_eq!(reception.merged_at.as_deref(), Some("2026-09-04T10:00:00Z"));
        assert_eq!(reception.minutes_to_merge, Some(60));
        assert_eq!(reception.review_cycles, 1);
        assert_eq!(reception.approvals, 2);
        assert_eq!(
            reception.comments,
            vec!["needs a regression test", "rename this variable"],
            "review bodies and inline comments both carry reviewer corrections"
        );
    }

    #[tokio::test]
    async fn an_unmerged_pull_request_reports_no_merge_time() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "created_at": "2026-09-04T09:00:00Z",
                "merged_at": null,
                "state": "open",
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let reception = PrService::standing_in_for("github.com", server.uri())
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .unwrap();

        assert_eq!(reception.merged_at, None);
        assert_eq!(reception.minutes_to_merge, None);
        assert_eq!(reception.state.as_deref(), Some("open"));
        assert!(reception.comments.is_empty());
    }

    #[tokio::test]
    async fn mergeability_distinguishes_conflicted_from_not_yet_computed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for (reported, expected) in [
            (serde_json::json!(true), Mergeability::Clean),
            (serde_json::json!(false), Mergeability::Conflicted),
            (serde_json::json!(null), Mergeability::Unknown),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/pulls/7"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({ "mergeable": reported })),
                )
                .mount(&server)
                .await;

            let mergeability = PrService::standing_in_for("github.com", server.uri())
                .fetch_mergeability(
                    &PullRequestReference {
                        owner: "acme".to_string(),
                        repository: "project".to_string(),
                        number: 7,
                    },
                    "token",
                )
                .await
                .unwrap();

            assert_eq!(mergeability, expected, "GitHub reported {reported}");
        }

        assert!(Mergeability::Conflicted.conflicted());
        assert!(
            !Mergeability::Unknown.conflicted(),
            "a branch nobody has checked is not a branch known to conflict"
        );
    }

    #[tokio::test]
    async fn a_rejected_token_is_reported_as_an_authentication_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let failure = PrService::standing_in_for("github.com", server.uri())
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .expect_err("an unauthorised read must not look like an empty pull request");

        assert!(matches!(failure, PrError::AuthFailed));
    }
}
