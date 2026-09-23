use serde::Serialize;

/// The body of a request to open a pull request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CreateRequest<'a> {
    pub(super) title: &'a str,
    pub(super) body: &'a str,
    pub(super) head: &'a str,
    pub(super) base: &'a str,
    pub(super) draft: bool,
}
