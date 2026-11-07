//! How visible a chunked symbol is outside its scope.

/// How visible a chunked symbol is outside its scope.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Visibility {
    /// Public or exported.
    Public,
    /// Private, and the default when no modifier says otherwise.
    #[default]
    Private,
    /// Protected.
    Protected,
    /// Internal to its module or package.
    Internal,
    /// Visible within its package.
    Package,
}
