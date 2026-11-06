use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Refresh<'a> {
    pub(super) grant_type: &'static str,
    pub(super) refresh_token: &'a str,
    pub(super) client_id: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub(super) scope: &'a str,
}
