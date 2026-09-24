use serde::Deserialize;

/// The repository a GraphQL query asked for, which GitHub leaves null when the
/// token cannot see it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlRepository<T> {
    pub(super) repository: Option<T>,
}
