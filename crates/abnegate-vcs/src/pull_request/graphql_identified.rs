/// A review thread as one page of a GraphQL answer holds it, known by the
/// node id that tells a thread met twice from two threads.
pub(super) trait GraphQlIdentified {
    fn id(&self) -> &str;
}
