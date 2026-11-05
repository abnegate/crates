use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct Request<'a> {
    pub(super) model: &'a str,
    pub(super) input: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) dimensions: Option<u32>,
}
