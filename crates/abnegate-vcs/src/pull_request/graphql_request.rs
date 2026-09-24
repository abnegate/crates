use serde::Serialize;

/// A GraphQL document and the values it reads, as GitHub's GraphQL endpoint
/// takes them.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GraphQlRequest<'a, V: ?Sized> {
    pub(super) query: &'static str,
    pub(super) variables: &'a V,
}
