use abnegate_secret::SecretValue;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest as _;
use sha2::Sha256;

use crate::login::LoginError;
use crate::login::claude::Endpoints;
use crate::login::claude::Redirect;
use crate::login::claude::Scope;

const VERIFIER_BYTES: usize = 64;
const STATE_BYTES: usize = 32;
const CHALLENGE_METHOD: &str = "S256";

/// The PKCE authorization a Claude sign-in starts from: the link to open,
/// and what the exchange of its code needs again.
///
/// `Debug` hides the verifier. Keep the authorization until its code comes
/// back, and match the code's state against `state`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Authorization {
    /// The authorize link, with the parameters the claude CLI sends in the
    /// order it sends them.
    pub url: String,
    /// The state the link carries, 32 random bytes in unpadded base64url.
    pub state: String,
    /// The PKCE verifier, 64 random bytes in unpadded base64url, whose
    /// SHA-256 challenge the link carries.
    pub verifier: SecretValue,
    /// What the sign-in asks for.
    pub scope: Scope,
    /// Where claude.com sends the browser once the sign-in is approved.
    pub redirect: Redirect,
}

impl Authorization {
    /// Draw a fresh verifier and state and build the link that asks
    /// `endpoints` for `scope`, sending the browser to `redirect`.
    pub fn new(
        endpoints: &Endpoints,
        scope: Scope,
        redirect: Redirect,
    ) -> Result<Self, LoginError> {
        let verifier = SecretValue::new(random::<VERIFIER_BYTES>()?);
        let state = random::<STATE_BYTES>()?;
        let mut url = endpoints.authorize().clone();
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", endpoints.client_id())
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect.uri(endpoints))
            .append_pair("scope", &scope.parameter())
            .append_pair("code_challenge", &challenge(verifier.expose()))
            .append_pair("code_challenge_method", CHALLENGE_METHOD)
            .append_pair("state", &state);

        Ok(Self {
            url: url.into(),
            state,
            verifier,
            scope,
            redirect,
        })
    }
}

fn random<const BYTES: usize>() -> Result<String, LoginError> {
    let mut bytes = [0u8; BYTES];
    getrandom::fill(&mut bytes).map_err(|_| LoginError::Entropy)?;
    Ok(encode(bytes))
}

fn challenge(verifier: &str) -> String {
    encode(Sha256::digest(verifier))
}

fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    use super::*;
    use crate::login::claude::AUTHORIZE_URL;
    use crate::login::claude::CLIENT_ID;
    use crate::login::claude::Flow;
    use crate::login::claude::REDIRECT_URL;

    const RFC_7636_VERIFIER_OCTETS: &[u8] = &[
        116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37,
        77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
    ];
    const INFERENCE_SCOPE: &str = "user:inference user:profile";
    const FULL_SCOPE: &str = "org:create_api_key user:profile user:inference \
                              user:sessions:claude_code user:mcp_servers user:file_upload \
                              user:plugins";
    const INFERENCE_QUERY: &str = "user%3Ainference+user%3Aprofile";
    const FULL_QUERY: &str = "org%3Acreate_api_key+user%3Aprofile+user%3Ainference\
                              +user%3Asessions%3Aclaude_code+user%3Amcp_servers\
                              +user%3Afile_upload+user%3Aplugins";

    fn authorization(scope: Scope, redirect: Redirect) -> Authorization {
        Authorization::new(&Endpoints::default(), scope, redirect).expect("random bytes")
    }

    #[test]
    fn the_rfc_7636_verifier_gives_its_challenge() {
        let verifier = encode(RFC_7636_VERIFIER_OCTETS);

        assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(
            challenge(&verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_authorize_url_carries_the_clis_parameters_in_the_clis_order() {
        for (scope, requested, encoded) in [
            (Scope::Inference, INFERENCE_SCOPE, INFERENCE_QUERY),
            (Scope::Full, FULL_SCOPE, FULL_QUERY),
        ] {
            let authorization = authorization(scope, Redirect::Paste);
            let code_challenge = challenge(authorization.verifier.expose());
            let state = authorization.state.as_str();
            let url = Url::parse(&authorization.url).expect("an absolute URL");
            let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();

            assert_eq!(authorization.scope, scope);
            assert_eq!(
                pairs,
                [
                    ("code", "true"),
                    ("client_id", CLIENT_ID),
                    ("response_type", "code"),
                    ("redirect_uri", REDIRECT_URL),
                    ("scope", requested),
                    ("code_challenge", code_challenge.as_str()),
                    ("code_challenge_method", "S256"),
                    ("state", state),
                ]
                .map(|(key, value)| (key.to_string(), value.to_string()))
            );
            assert_eq!(
                authorization.url,
                format!(
                    "{AUTHORIZE_URL}?code=true&client_id={CLIENT_ID}&response_type=code\
                     &redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback\
                     &scope={encoded}&code_challenge={code_challenge}\
                     &code_challenge_method=S256&state={state}"
                )
            );
        }
    }

    #[test]
    fn a_loopback_authorization_sends_the_browser_back_to_the_callers_listener() {
        let authorization = authorization(Scope::Inference, Redirect::Loopback(54_545));
        let url = Url::parse(&authorization.url).expect("an absolute URL");
        let redirect: Vec<String> = url
            .query_pairs()
            .filter(|(key, _)| key == "redirect_uri")
            .map(|(_, value)| value.into_owned())
            .collect();

        assert_eq!(redirect, ["http://localhost:54545/callback"]);
        assert_eq!(authorization.redirect, Redirect::Loopback(54_545));
        assert_eq!(authorization.redirect.flow(), Flow::Loopback);
        assert!(
            authorization.url.contains(
                "&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&scope="
            ),
            "{}",
            authorization.url
        );
    }

    #[test]
    fn an_authorization_asks_the_endpoints_it_was_given() {
        let endpoints = Endpoints::default()
            .with_client_id("another-client")
            .and_then(|endpoints| endpoints.with_authorize("http://127.0.0.1:4010/authorize"))
            .expect("loopback endpoints");

        let authorization =
            Authorization::new(&endpoints, Scope::Inference, Redirect::Paste).expect("random");

        assert!(
            authorization
                .url
                .starts_with("http://127.0.0.1:4010/authorize?code=true&client_id=another-client&"),
            "{}",
            authorization.url
        );
    }

    #[test]
    fn each_authorization_draws_a_fresh_verifier_and_state() {
        let first = authorization(Scope::Inference, Redirect::Paste);
        let second = authorization(Scope::Inference, Redirect::Paste);
        let url_safe = |text: &str| {
            text.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        };

        assert_eq!(
            first.verifier.expose().len(),
            86,
            "64 random bytes in unpadded base64url"
        );
        assert_eq!(
            first.state.len(),
            43,
            "32 random bytes in unpadded base64url"
        );
        assert!(url_safe(first.verifier.expose()) && url_safe(&first.state));
        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.state, second.state);
    }

    #[test]
    fn debug_of_an_authorization_hides_the_verifier() {
        let authorization = authorization(Scope::Full, Redirect::Loopback(54_545));
        let rendered = format!("{authorization:?}");

        assert!(
            !rendered.contains(authorization.verifier.expose()),
            "{rendered}"
        );
        assert!(rendered.contains(&authorization.state), "{rendered}");
    }
}
