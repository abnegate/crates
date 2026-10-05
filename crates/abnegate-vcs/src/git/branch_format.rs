/// How [`GitService::generate_branch_name`](crate::GitService::generate_branch_name)
/// joins its prefix to the task's short identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum BranchFormat {
    /// `{prefix}/{short id}-{slug}`: every task's branch sits in the prefix's
    /// namespace, as `task/1a2b3c4d-add-rate-limiting`.
    #[default]
    Namespaced,
    /// `{prefix}-{short id}-{slug}`: the prefix leads the branch's own last
    /// segment, as `zone/task-1a2b3c4d-add-rate-limiting` under `zone/task`.
    Joined,
}

impl BranchFormat {
    /// What stands between the prefix and the short identifier.
    pub(crate) fn separator(self) -> char {
        match self {
            Self::Namespaced => '/',
            Self::Joined => '-',
        }
    }
}
