use serde::Serialize;

/// One source entry, or the fragment of it that fits the request.
#[derive(Clone, Serialize)]
pub(super) struct Source<'a> {
    pub(super) id: &'a str,
    pub(super) offset: usize,
    pub(super) total_bytes: usize,
    pub(super) fragment: &'a str,
}
