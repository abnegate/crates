use std::fs::OpenOptions;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nix::fcntl::OFlag;
use serde_json::Value;

use crate::login::codex::CREDENTIALS;
use crate::login::codex::Plan;

const TOKENS: &str = "tokens";
const ID_TOKEN: &str = "id_token";
const ACCOUNT_ID: &str = "account_id";
const EMAIL: &str = "email";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";
const PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const ACCOUNT_CLAIM: &str = "chatgpt_account_id";
const PLAN_CLAIM: &str = "chatgpt_plan_type";
const LIMIT: u64 = 64 * 1024;

/// Whose ChatGPT login codex saved, read from its `auth.json` and the id
/// token in it.
///
/// The id token is read, never verified: codex received it from OpenAI over
/// TLS, and an account only labels a login and tells logins apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Account {
    /// The ChatGPT account the login works in, which several people of one
    /// team share.
    pub id: String,
    /// The email of the person signed in, when the id token names one.
    pub email: Option<String>,
    /// The ChatGPT plan as the id token spells it, such as `pro`.
    pub subscription: Option<String>,
}

impl Account {
    /// The ChatGPT account `id`, with no email or plan known.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            email: None,
            subscription: None,
        }
    }

    /// This account, signed in as `email`.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// This account, on the plan `subscription` spells.
    pub fn with_subscription(mut self, subscription: impl Into<String>) -> Self {
        self.subscription = Some(subscription.into());
        self
    }

    /// The account of the login in `home`, when its `auth.json` is a regular
    /// file of at most 64 KiB that names a ChatGPT account. A link in its
    /// place is never followed, and a FIFO is never waited on.
    pub fn read(home: &Path) -> Option<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(home.join(CREDENTIALS))
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut auth = String::new();
        file.take(LIMIT + 1).read_to_string(&mut auth).ok()?;
        if auth.len() as u64 > LIMIT {
            return None;
        }
        Self::parse(&auth)
    }

    /// The account of the login in `auth`, the text of codex's `auth.json`,
    /// or `None` when it names no ChatGPT account. The account id codex
    /// sends its requests for comes before the one the id token names.
    pub fn parse(auth: &str) -> Option<Self> {
        let auth: Value = serde_json::from_str(auth).ok()?;
        let tokens = auth.get(TOKENS)?;
        let claims = tokens
            .get(ID_TOKEN)
            .and_then(Value::as_str)
            .and_then(claims);
        let granted = claims.as_ref().and_then(|claims| claims.get(AUTH_CLAIM));
        let id = text(tokens.get(ACCOUNT_ID))
            .or_else(|| text(granted.and_then(|granted| granted.get(ACCOUNT_CLAIM))))?;
        let email = claims.as_ref().and_then(|claims| {
            text(claims.get(EMAIL)).or_else(|| {
                text(
                    claims
                        .get(PROFILE_CLAIM)
                        .and_then(|profile| profile.get(EMAIL)),
                )
            })
        });
        let subscription = text(granted.and_then(|granted| granted.get(PLAN_CLAIM)));
        Some(Self {
            id,
            email,
            subscription,
        })
    }

    /// What tells one login from another: one person in one ChatGPT account
    /// is one login, whichever of the account's members or the person's
    /// accounts the others are, and whatever plan it is on.
    pub fn key(&self) -> String {
        match &self.email {
            Some(email) => format!("{}/{email}", self.id),
            None => self.id.clone(),
        }
    }

    /// The plan `subscription` names, when it is one ChatGPT sells.
    pub fn plan(&self) -> Option<Plan> {
        self.subscription.as_deref().and_then(Plan::named)
    }
}

/// The claims of the JSON web token `token`.
fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?.trim_end_matches('=');
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ACCOUNT: &str = "00000000-0000-4000-8000-000000000000";
    const TEAM: &str = "11111111-1111-4111-8111-111111111111";
    const PERSON: &str = "person@example.com";

    fn token(claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("{header}.{payload}.signature")
    }

    fn auth(tokens: &Value) -> String {
        json!({ "auth_mode": "chatgpt", "OPENAI_API_KEY": null, "tokens": tokens }).to_string()
    }

    #[test]
    fn an_account_is_read_from_auth_json_and_its_id_token() {
        let claims = json!({
            "email": PERSON,
            AUTH_CLAIM: { ACCOUNT_CLAIM: TEAM, PLAN_CLAIM: "pro" },
        });
        let saved = auth(&json!({
            "id_token": token(&claims),
            "access_token": "fake-access",
            "refresh_token": "fake-refresh",
            "account_id": ACCOUNT,
        }));

        let account = Account::parse(&saved);
        assert_eq!(
            account,
            Some(
                Account::new(ACCOUNT)
                    .with_email(PERSON)
                    .with_subscription("pro")
            ),
            "the account codex sends requests for comes before the one the token names"
        );
        assert_eq!(account.and_then(|account| account.plan()), Some(Plan::Pro));

        let unnamed = auth(&json!({ "id_token": token(&claims) }));
        assert_eq!(
            Account::parse(&unnamed).map(|account| account.id),
            Some(TEAM.to_string()),
            "an auth.json without its own account id falls back to the token's"
        );

        let profiled = auth(&json!({
            "id_token": token(&json!({ PROFILE_CLAIM: { "email": PERSON } })),
            "account_id": ACCOUNT,
        }));
        assert_eq!(
            Account::parse(&profiled).and_then(|account| account.email),
            Some(PERSON.to_string()),
            "an email kept only in the profile claim was missed"
        );
    }

    #[test]
    fn an_auth_json_whose_id_token_is_unreadable_still_names_its_account() {
        let saved = auth(&json!({ "id_token": "fixture", "account_id": ACCOUNT }));

        assert_eq!(Account::parse(&saved), Some(Account::new(ACCOUNT)));
    }

    #[test]
    fn an_auth_json_that_names_no_account_has_none() {
        for (case, saved) in [
            "{}".to_string(),
            "not json".to_string(),
            json!({ "OPENAI_API_KEY": "fake-key", "tokens": null }).to_string(),
            auth(&json!({ "id_token": "fixture", "account_id": "" })),
            auth(&json!({ "id_token": token(&json!({ "email": PERSON })) })),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(Account::parse(&saved), None, "case {case}");
        }
    }

    #[test]
    fn only_a_regular_auth_json_within_the_limit_is_read() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let home = directory.path();
        let saved = auth(&json!({ "id_token": "fixture", "account_id": ACCOUNT }));
        let path = home.join(CREDENTIALS);

        std::fs::write(&path, &saved).expect("a login");
        assert_eq!(Account::read(home), Some(Account::new(ACCOUNT)));

        let padded = format!(
            "{saved}{}",
            " ".repeat(usize::try_from(LIMIT).expect("small"))
        );
        assert!(
            Account::parse(&padded).is_some(),
            "the padding is still JSON"
        );
        std::fs::write(&path, padded).expect("an oversized login");
        assert_eq!(Account::read(home), None, "an oversized auth.json was read");

        std::fs::remove_file(&path).expect("the login removed");
        let elsewhere = directory.path().join("elsewhere.json");
        std::fs::write(&elsewhere, &saved).expect("a login elsewhere");
        std::os::unix::fs::symlink(&elsewhere, &path).expect("a link in its place");
        assert_eq!(Account::read(home), None, "a link was followed");

        std::fs::remove_file(&path).expect("the link removed");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).expect("a FIFO in its place");
        assert_eq!(Account::read(home), None, "a FIFO was read");
    }

    #[test]
    fn one_person_in_one_account_is_one_key() {
        let person = Account::new(ACCOUNT)
            .with_email(PERSON)
            .with_subscription("pro");
        let teammate = person.clone().with_email("ada@example.com");
        let elsewhere = Account {
            id: TEAM.to_string(),
            ..person.clone()
        };
        let upgraded = person.clone().with_subscription("team");

        assert_eq!(
            person.key(),
            upgraded.key(),
            "a plan change made a new login"
        );
        assert_ne!(
            person.key(),
            teammate.key(),
            "two people of one team merged"
        );
        assert_ne!(
            person.key(),
            elsewhere.key(),
            "one person's two accounts merged"
        );
        assert_eq!(Account::new(ACCOUNT).key(), ACCOUNT);
    }
}
