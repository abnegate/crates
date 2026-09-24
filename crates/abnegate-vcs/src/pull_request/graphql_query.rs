use serde::Deserialize;

/// The query root of a GraphQL answer, holding the repository asked for,
/// which GitHub leaves null when the token cannot see it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlQuery<T> {
    pub(super) repository: Option<T>,
}
