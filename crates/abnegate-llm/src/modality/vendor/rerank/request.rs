use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct Request<'a> {
    pub(super) model: &'a str,
    pub(super) query: &'a str,
    pub(super) documents: &'a [String],
}
