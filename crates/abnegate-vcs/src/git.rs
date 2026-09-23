//! Git operations, run by shelling out to `git`.
//!
//! [`GitService`] covers two kinds of repository. A *hardened* one is reached
//! over HTTPS at github.com, cloned and pushed with a credential that never
//! enters the URL, the process table or `.git/config`, and run with every
//! setting a repository could name a program through pinned on the command
//! line. A *managed* one is a local clone the caller owns outright: it is
//! cloned, fetched, reset and given worktrees from whatever address the caller
//! configured, including a local path.

mod authentication;
mod diff_summary;
mod error;
#[cfg(unix)]
mod group;
mod remote_head;
mod service;

pub(crate) use crate::git::authentication::authenticate;
pub use crate::git::diff_summary::DiffSummary;
pub use crate::git::error::GitError;
pub use crate::git::error::GitResult;
pub use crate::git::remote_head::RemoteHead;
pub use crate::git::service::GitService;
