use abnegate_secret::SecretValue;
use reqwest::Url;

use crate::login::LoginError;
use crate::login::claude::CALLBACK_PATH;
use crate::login::claude::Endpoints;
use crate::login::claude::LOOPBACK_HOST;
use crate::login::claude::LOOPBACK_SCHEME;
use crate::login::claude::reply::single;

const SEPARATOR: char = '#';
const UNREADABLE: &str = "Paste the code Claude showed, as code#state, or the whole callback URL";
const FOREIGN: &str = "That URL is not Claude's sign-in callback";
const DECLINED: &str =
    "Claude did not approve the sign-in. Open the link, approve it, then paste the new code";
const INCOMPLETE: &str = "The callback URL is missing its code or its state";

/// The code an approved Claude sign-in hands back, and the state it answers.
///
/// Compare `state` with the [`Authorization`](crate::login::claude::Authorization)'s
/// own before exchanging the code: the token endpoint checks the PKCE
/// verifier, not which sign-in a code was meant for.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Code {
    /// The authorization code.
    pub value: SecretValue,
    /// The state the authorize link carried.
    pub state: String,
}

impl Code {
    /// A code and the state it answers, each non-empty and free of
    /// whitespace and `#`.
    ///
    /// # Errors
    ///
    /// [`LoginError::Malformed`] when either is empty or holds whitespace or
    /// `#`.
    pub fn new(value: &str, state: &str) -> Result<Self, LoginError> {
        let clean = |part: &str| {
            !part.is_empty() && !part.contains(SEPARATOR) && !part.contains(char::is_whitespace)
        };
        if !clean(value) || !clean(state) {
            return Err(LoginError::Malformed(UNREADABLE));
        }
        Ok(Self {
            value: SecretValue::new(value),
            state: state.to_string(),
        })
    }

    /// The code someone pasted back: `code#state` as claude.com's page shows
    /// it, or the whole address of a callback, either the
    /// [`Endpoints::redirect`] page or a loopback one the browser could not
    /// reach. Surrounding whitespace is ignored; a callback that names an
    /// `error` is refused as declined, so the same link can be approved
    /// again.
    ///
    /// # Errors
    ///
    /// [`LoginError::Malformed`] when the paste is neither `code#state` nor
    /// a callback address, when it is the address of a page other than the
    /// redirect or a loopback callback, when the callback was declined, and
    /// when it lacks its code or its state. No message quotes the paste.
    pub fn parse(endpoints: &Endpoints, pasted: &str) -> Result<Self, LoginError> {
        let pasted = pasted.trim();
        match Url::parse(pasted) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                Self::from_callback(endpoints, &url)
            }
            _ => {
                let (value, state) = pasted
                    .split_once(SEPARATOR)
                    .ok_or(LoginError::Malformed(UNREADABLE))?;
                Self::new(value, state)
            }
        }
    }

    fn from_callback(endpoints: &Endpoints, url: &Url) -> Result<Self, LoginError> {
        let mut callback = url.clone();
        callback.set_query(None);
        callback.set_fragment(None);
        let loopback = callback.scheme() == LOOPBACK_SCHEME
            && callback.host_str() == Some(LOOPBACK_HOST)
            && callback.port().is_some()
            && callback.path() == CALLBACK_PATH
            && callback.username().is_empty()
            && callback.password().is_none();
        if &callback != endpoints.redirect() && !loopback {
            return Err(LoginError::Malformed(FOREIGN));
        }

        let pairs = url.query_pairs().into_owned().collect::<Vec<_>>();
        if pairs.iter().any(|(key, _)| key == "error") {
            return Err(LoginError::Malformed(DECLINED));
        }
        match (single(&pairs, "code"), single(&pairs, "state")) {
            (Some(value), Some(state)) => Self::new(value, state),
            _ => Err(LoginError::Malformed(INCOMPLETE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASTED_URL: &str =
        "https://platform.claude.com/oauth/code/callback?code=fake-code&state=fake-state";

    fn code() -> Code {
        Code {
            value: SecretValue::new("fake-code"),
            state: "fake-state".to_string(),
        }
    }

    fn parse(pasted: &str) -> Result<Code, LoginError> {
        Code::parse(&Endpoints::default(), pasted)
    }

    fn refusal(pasted: &str) -> &'static str {
        match parse(pasted) {
            Err(LoginError::Malformed(message)) => message,
            other => panic!("{pasted:?} must be refused as malformed, got {other:?}"),
        }
    }

    #[test]
    fn a_pasted_code_splits_into_its_code_and_state() {
        assert_eq!(parse("fake-code#fake-state").expect("parses"), code());
    }

    #[test]
    fn a_pasted_callback_url_gives_up_its_code_and_state() {
        assert_eq!(parse(PASTED_URL).expect("parses"), code());
        assert_eq!(
            parse(
                &format!("{PASTED_URL}#fragment").replace("https://platform", "HTTPS://PLATFORM")
            )
            .expect("scheme and host case do not matter"),
            code()
        );
    }

    #[test]
    fn a_paste_without_a_separator_or_with_an_empty_half_is_refused() {
        for pasted in [
            "fake-code",
            "#fake-state",
            "fake-code#",
            "#",
            "fake-code#fake-state#extra",
        ] {
            assert_eq!(refusal(pasted), UNREADABLE, "{pasted:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_and_inner_whitespace_refused() {
        assert_eq!(
            parse("  fake-code#fake-state\n").expect("the paste is trimmed"),
            code()
        );
        for pasted in [
            "fake-code #fake-state",
            "fake code#fake-state",
            "fake-code#fake\tstate",
            "",
            " \n\t ",
        ] {
            assert_eq!(refusal(pasted), UNREADABLE, "{pasted:?}");
        }
    }

    #[test]
    fn the_address_of_a_callback_the_browser_could_not_reach_gives_up_its_code_and_state() {
        for pasted in [
            "http://localhost:54545/callback?code=fake-code&state=fake-state",
            "http://LOCALHOST:1/callback?state=fake-state&code=fake-code#fragment",
        ] {
            assert_eq!(parse(pasted).expect("parses"), code(), "{pasted}");
        }
    }

    #[test]
    fn a_url_other_than_claudes_callback_is_refused() {
        for pasted in [
            "https://attacker.example/oauth/code/callback?code=fake-code&state=fake-state",
            "http://platform.claude.com/oauth/code/callback?code=fake-code&state=fake-state",
            "https://platform.claude.com.attacker.example/oauth/code/callback?code=fake-code&state=fake-state",
            "https://platform.claude.com/oauth/code/callback/more?code=fake-code&state=fake-state",
            "https://someone@platform.claude.com/oauth/code/callback?code=fake-code&state=fake-state",
            "https://localhost:54545/callback?code=fake-code&state=fake-state",
            "http://127.0.0.1:54545/callback?code=fake-code&state=fake-state",
            "http://localhost/callback?code=fake-code&state=fake-state",
            "http://localhost:54545/other?code=fake-code&state=fake-state",
            "http://someone@localhost:54545/callback?code=fake-code&state=fake-state",
            "http://localhost.attacker.example:54545/callback?code=fake-code&state=fake-state",
        ] {
            assert_eq!(refusal(pasted), FOREIGN, "{pasted}");
        }
    }

    #[test]
    fn a_callback_without_a_code_is_refused_and_a_declined_one_says_so() {
        assert_eq!(
            refusal("https://platform.claude.com/oauth/code/callback?state=fake-state"),
            INCOMPLETE
        );
        assert_eq!(
            refusal(
                "https://platform.claude.com/oauth/code/callback\
                 ?error=access_denied&state=fake-state"
            ),
            DECLINED
        );
    }

    #[test]
    fn a_declined_callback_asks_for_the_same_link_to_be_approved() {
        let message = refusal(
            "https://platform.claude.com/oauth/code/callback\
             ?error=access_denied&state=fake-state",
        )
        .to_ascii_lowercase();

        assert!(!message.contains("start"), "{message}");
        for step in ["open the link", "approve it", "paste the new code"] {
            assert!(message.contains(step), "{step:?} missing from {message:?}");
        }
    }

    #[test]
    fn a_callback_is_read_against_the_redirect_the_endpoints_name() {
        let endpoints = Endpoints::default()
            .with_redirect("http://127.0.0.1:4010/oauth/code/callback")
            .expect("a loopback redirect");

        assert_eq!(
            Code::parse(
                &endpoints,
                "http://127.0.0.1:4010/oauth/code/callback?code=fake-code&state=fake-state"
            )
            .expect("the configured page"),
            code()
        );
        assert!(matches!(
            Code::parse(&endpoints, PASTED_URL),
            Err(LoginError::Malformed(FOREIGN))
        ));
    }

    #[test]
    fn a_callback_naming_its_code_twice_is_incomplete() {
        assert_eq!(
            refusal(
                "https://platform.claude.com/oauth/code/callback\
                 ?code=fake-code&code=other-code&state=fake-state"
            ),
            INCOMPLETE
        );
    }
}
