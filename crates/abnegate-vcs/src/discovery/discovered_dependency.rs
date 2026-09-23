use crate::discovery::Manifest;

/// One dependency one repository declares on another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDependency {
    /// The repository that has the dependency.
    pub repository: String,
    /// The dependency's package name, as `organisation/package`.
    pub depends_on: String,
    /// The package manager it was declared to.
    pub manifest: Manifest,
    /// Where on disk the repository declaring it was found.
    pub repository_path: String,
}
