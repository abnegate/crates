use crate::pull_request::MergeMethod;

/// How a repository lets its pull requests merge.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MergeSettings {
    /// Whether a pull request may be set to merge once it is ready.
    pub auto_merge: bool,
    /// The methods a pull request may merge with, in the order squash, merge,
    /// rebase.
    pub methods: Vec<MergeMethod>,
}

impl MergeSettings {
    /// A repository that allows auto-merge when `auto_merge` says so, and
    /// merging with `methods`.
    pub fn new(auto_merge: bool, methods: Vec<MergeMethod>) -> Self {
        Self {
            auto_merge,
            methods,
        }
    }

    /// Whether a pull request may merge with `method`.
    pub fn allows(&self, method: MergeMethod) -> bool {
        self.methods.contains(&method)
    }
}
