//! Repository reads from GitLab, on gitlab.com or a self-hosted instance.
//!
//! Only the `gitlab` feature builds this module. [`GitLabService`] reads what
//! indexing a project's files takes, and nothing else:
//! [`GitLabService::get_default_branch`] reads the branch a project builds
//! from, [`GitLabService::fetch_tree_at`] lists every entry of its tree at a
//! branch, tag or commit, beneath a [`RepositoryPath`](crate::RepositoryPath)
//! when one is given, walking every page GitLab answers with, and
//! [`GitLabService::fetch_blob`] reads one blob's bytes, as they are, up to
//! the limit its caller passes.
//!
//! A project is named by a [`Project`], its path or its numeric id, and
//! travels as one percent-encoded path segment, so `group/sub` is sent as
//! `group%2Fsub`. A branch, tag or commit travels as the `ref` query value,
//! held first to the rules git holds a ref's name to, a directory travels as
//! the `path` query value, and a blob is named by its full object name. So no
//! value reaches another endpoint or adds a parameter.
//!
//! The service addresses gitlab.com by default, or the instance an operator
//! configures, over HTTPS unless plain HTTP was opted in to for a loopback or
//! LAN host. It never reaches a link-local address or a cloud provider's
//! metadata service, whether the configured URL names one or a name resolves
//! to one. It ignores every proxy unless
//! [`GitLabService::with_system_proxy`] opts in to the one the environment or
//! the operating system names, which then resolves the instance's name
//! itself, so only the configured URL is checked. It follows a redirect only
//! within the configured origin, and it
//! takes nothing but the query from a page's link to the next, asking for
//! that page where it asked for the one before, so the token never travels
//! anywhere else.

mod error;
mod listed_entry;
mod origin;
mod project;
mod project_detail;
mod service;

pub use crate::gitlab::error::GitLabError;
pub use crate::gitlab::error::GitLabResult;
pub use crate::gitlab::project::Project;
pub use crate::gitlab::service::GitLabService;
