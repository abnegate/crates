#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Version control over the `git` command line.
//!
//! [`git::GitService`] shells out to `git` for the clone, branch, commit, push
//! and worktree a change needs, in a [`Checkout`] named by its top and bound
//! to the clone it was made from; [`worktree`] adds and removes the detached
//! worktrees a batch of concurrent runs works in; [`conflict::ConflictService`]
//! reproduces a pull request's merge conflict in a throwaway checkout, or
//! refreshes a branch by merging its base in there and pushing the merge, and
//! [`resolution::judge`] refuses a repair that threw a branch's work away;
//! [`subject::Subject`] names a change the way a conventional-commit history
//! names one; [`discovery::DependencyDiscovery`] reads package manifests for
//! the dependencies one organisation has on itself. With the `github` feature,
//! `pull_request::PullRequestService` opens, reads, checks, reviews and merges
//! pull requests, sets them to merge once they are ready, moves them in and
//! out of draft, creates repositories and registers their webhooks, over
//! GitHub's REST and GraphQL APIs.
//!
//! Nothing here holds application state, so a task runner can drive it directly.
//!
//! The crate needs git 2.39 or newer, the first to read a symbolic ref without
//! following it: on an older git [`GitService::commit`] fails closed and
//! [`worktree::branch`] reads a worktree's branch as unknown. Its test suite
//! needs git 2.45 or newer, the first to take `--ref-format` for `init` and
//! `clone`.
//!
//! ```no_run
//! # async fn example(repository: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_vcs::GitService;
//! use uuid::Uuid;
//!
//! let git = GitService::new();
//! let branch = git.generate_branch_name(Uuid::new_v4(), "add rate limiting")?;
//! # let _ = (git.is_git_repository(repository).await?, branch);
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `github`: `pull_request`, which opens, reads, checks, reviews and merges
//!   pull requests on GitHub or a GitHub Enterprise install, sets them to
//!   merge once they are ready and moves them in and out of draft, reads back
//!   how each one was received, and creates repositories there and registers their
//!   webhooks, over the REST and GraphQL APIs; and
//!   `pull_request::WebhookSignature`, which checks that a webhook delivery was
//!   signed with the secret its webhook was registered with.
//! - `github-app`: `github`, plus `pull_request::ApplicationClaims`, which
//!   signs the JWT that authenticates as a GitHub App,
//!   `pull_request::InstallationIssuer`, which trades it for an installation
//!   access token, `pull_request::TokenCache`, which holds each
//!   installation's token until it nears expiry, and
//!   `pull_request::ApplicationService`, which registers an App from its
//!   manifest and lists its installations and their repositories.
//! - `testing`: `RepositoryUrl::local`,
//!   `pull_request::PullRequestService::standing_in_for` and, with
//!   `github-app`, `pull_request::InstallationIssuer::standing_in_for` and
//!   `pull_request::ApplicationService::standing_in_for`, which reach a
//!   repository on the local disk or a mock API server. Nothing a
//!   caller configures produces any of them, so only a test should enable it.

mod branch_name;
mod checkout;
mod commit_sha;
pub mod conflict;
pub mod discovery;
pub mod git;
mod object_id;
mod parse_error;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub mod pull_request;
mod repository_url;
pub mod resolution;
pub mod subject;
mod tag_name;
mod truncation;
pub mod worktree;

pub use crate::branch_name::BranchName;
pub use crate::checkout::Checkout;
pub use crate::commit_sha::CommitSha;
pub use crate::conflict::Conflict;
pub use crate::conflict::ConflictError;
pub use crate::conflict::ConflictRequest;
pub use crate::conflict::ConflictService;
pub use crate::conflict::ConflictedPath;
pub use crate::conflict::Refreshed;
pub use crate::discovery::DependencyDiscovery;
pub use crate::discovery::DiscoveredDependency;
pub use crate::discovery::Manifest;
pub use crate::git::BranchFormat;
pub use crate::git::DiffSummary;
pub use crate::git::GitError;
pub use crate::git::GitService;
pub use crate::git::RemoteHead;
pub use crate::object_id::ObjectId;
pub use crate::parse_error::ParseError;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::Account;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::AccountType;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::ApplicationClaims;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::ApplicationError;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::ApplicationId;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::ApplicationService;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::AutoMerge;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Blob;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ChangedFile;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::CheckRun;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ChecksOutcome;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Collected;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::CommitStatus;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::CreatedPullRequest;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::CreatedRepository;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Deployment;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::DeploymentStatus;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Description;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::EnabledAutoMerge;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Excerpt;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::FileStatus;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::FoundPullRequest;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::GitHubBranch;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::GitHubPullRequest;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::Installation;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::InstallationId;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::InstallationIssuer;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::InstallationToken;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::InstalledRepository;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Issue;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::IssueComment;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::IssueReference;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::IssueState;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::IssueUpdate;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::JobLog;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::JobLogError;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Label;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ListedPullRequest;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::MergeMethod;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::MergeSettings;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Mergeability;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::MergeableState;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::MergedPullRequest;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Page;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestAutomation;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestDetail;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestError;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestReception;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestReference;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestService;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::PullRequestState;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Release;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Repository;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::RepositoryPath;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::RepositorySelection;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ReviewEvent;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ReviewRecord;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ReviewState;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ReviewTally;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ReviewThreadRecord;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::StateFilter;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::SubmittedReview;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::ThreadComment;
#[cfg(feature = "github-app")]
#[cfg_attr(docsrs, doc(cfg(feature = "github-app")))]
pub use crate::pull_request::TokenCache;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::Tree;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::TreeEntry;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::TreeEntryKind;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::WebhookEvent;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::WebhookSignature;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::WorkflowJob;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::WorkflowRun;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::WorkflowRunFilter;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::minutes_between;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::tally;
pub use crate::repository_url::RepositoryUrl;
pub use crate::resolution::ConflictSide;
pub use crate::resolution::ResolutionVerdict;
pub use crate::resolution::judge;
pub use crate::subject::Kind;
pub use crate::subject::Subject;
pub use crate::worktree::Unfinished;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
