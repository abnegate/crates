#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Version control over the `git` command line.
//!
//! [`git::GitService`] shells out to `git` for the clone, branch, commit, push
//! and worktree a change needs; [`worktree`] adds and removes the detached
//! worktrees a batch of concurrent runs works in; [`conflict::ConflictService`]
//! reproduces a pull request's merge conflict in a throwaway checkout and
//! [`resolution::judge`] refuses a repair that threw a branch's work away;
//! [`subject::Subject`] names a change the way a conventional-commit history
//! names one; [`discovery::DependencyDiscovery`] reads package manifests for
//! the dependencies one organisation has on itself. With the `github` feature,
//! [`pull_request::PrService`] opens and reads back pull requests over the
//! GitHub REST API.
//!
//! Nothing here holds application state, so a task runner can drive it directly.
//!
//! ```no_run
//! # async fn example(repository: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_vcs::GitService;
//! use uuid::Uuid;
//!
//! let git = GitService::new();
//! let branch = git.generate_branch_name(Uuid::new_v4(), "add rate limiting")?;
//! # let _ = (git.is_git_repo(repository).await?, branch);
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `github`: [`pull_request`], which opens pull requests on GitHub or a
//!   GitHub Enterprise install and reads back how each one was received.

mod branch_name;
mod commit_sha;
pub mod conflict;
pub mod discovery;
pub mod git;
mod parse_error;
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub mod pull_request;
mod repository_url;
pub mod resolution;
pub mod subject;
pub mod worktree;

pub use crate::branch_name::BranchName;
pub use crate::commit_sha::CommitSha;
pub use crate::parse_error::ParseError;
pub use crate::repository_url::RepositoryUrl;

pub use crate::conflict::{
    Conflict, ConflictError, ConflictRequest, ConflictService, ConflictedPath,
};
pub use crate::discovery::{DependencyDiscovery, DiscoveredDependency, Manifest};
pub use crate::git::{DiffSummary, GitError, GitService, RemoteHead};
#[cfg(feature = "github")]
#[cfg_attr(docsrs, doc(cfg(feature = "github")))]
pub use crate::pull_request::{
    CreatedPr, Description, GitHubBranch, GitHubPullRequest, Mergeability, PrError, PrService,
    PullRequestReception, PullRequestReference, ReviewState, ReviewTally, SubmittedReview,
    minutes_between, tally,
};
pub use crate::resolution::{ConflictSide, ResolutionVerdict, judge};
pub use crate::subject::{Kind, Subject};
pub use crate::worktree::Unfinished;
