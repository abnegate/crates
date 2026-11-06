use abnegate_secret::SecretValue;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use serde::Deserialize;

use crate::login::LoginError;
use crate::login::claude::Tokens;
use crate::login::claude::client::UNREADABLE;

#[derive(Deserialize)]
pub(super) struct Granted {
    access_token: SecretValue,
    refresh_token: Option<SecretValue>,
    expires_in: u64,
    scope: Option<String>,
    subscription_type: Option<String>,
}

impl Granted {
    pub(super) fn tokens(self, now: DateTime<Utc>) -> Result<Tokens, LoginError> {
        let expires_at = i64::try_from(self.expires_in)
            .ok()
            .and_then(TimeDelta::try_seconds)
            .and_then(|lifetime| now.checked_add_signed(lifetime))
            .ok_or(LoginError::Malformed(UNREADABLE))?;
        if self.access_token.is_empty() {
            return Err(LoginError::Malformed(UNREADABLE));
        }
        Ok(Tokens::new(
            self.access_token,
            self.refresh_token.filter(|token| !token.is_empty()),
            expires_at,
            Some(now),
            self.scope.unwrap_or_default(),
            self.subscription_type.filter(|kind| !kind.is_empty()),
        ))
    }
}
