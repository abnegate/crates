use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Exchange<'a> {
    pub(super) grant_type: &'static str,
    pub(super) code: &'a str,
    pub(super) redirect_uri: &'a str,
    pub(super) client_id: &'a str,
    pub(super) code_verifier: &'a str,
    pub(super) state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expires_in: Option<u64>,
}
