use serde::Deserialize;

use crate::login::claude::Refusal;
use crate::login::claude::client::reason::Reason;

#[derive(Deserialize)]
pub(super) struct Failure {
    error: Option<Reason>,
    error_description: Option<String>,
}

impl Failure {
    pub(super) fn refusal(self) -> Option<Refusal> {
        match self.error? {
            Reason::Code(code) => Some(Refusal::named(&code, self.error_description.as_deref())),
            Reason::Detail(_) => None,
        }
    }
}
