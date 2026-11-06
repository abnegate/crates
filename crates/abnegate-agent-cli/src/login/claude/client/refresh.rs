use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Refresh<'a> {
    pub(super) grant_type: &'static str,
    pub(super) refresh_token: &'a str,
    pub(super) client_id: &'a str,
    pub(super) scope: &'a str,
}
