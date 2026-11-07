use serde::Deserialize;

/// The part of a project's details GitLab's REST API answers with that this
/// service reads. A project with an empty repository has no default branch.
#[derive(Debug, Deserialize)]
pub(super) struct GitLabProject {
    pub(super) default_branch: Option<String>,
}
