use abnegate_http::TargetPolicy;
use reqwest::Url;

use crate::login::LoginError;
use crate::login::claude::AUTHORIZE_URL;
use crate::login::claude::CLIENT_ID;
use crate::login::claude::REDIRECT_URL;
use crate::login::claude::TOKEN_URL;

const AUTHORIZE: &str = "authorize";
const TOKEN: &str = "token";
const REDIRECT: &str = "redirect";
const UNNAMED: &str = "The Claude client id must be non-empty and free of whitespace";

/// The OAuth client a Claude sign-in runs as and the URLs it reaches.
///
/// [`Endpoints::default`] is what the claude CLI uses. Each `with_*` builder
/// checks its value: a URL with [`TargetPolicy::ExceptMetadata`], which
/// refuses a scheme other than `http` or `https`, embedded credentials, a
/// missing host and a link-local or cloud metadata target, and still allows
/// loopback and LAN hosts, so a sign-in can run against a local stand-in.
///
/// ```
/// use abnegate_agent_cli::login::claude::Endpoints;
///
/// let endpoints = Endpoints::default().with_token("http://127.0.0.1:4010/v1/oauth/token")?;
/// assert_eq!(endpoints.token().as_str(), "http://127.0.0.1:4010/v1/oauth/token");
/// assert!(Endpoints::default().with_token("http://169.254.169.254/latest").is_err());
/// # Ok::<(), abnegate_agent_cli::login::LoginError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Endpoints {
    client_id: String,
    authorize: Url,
    token: Url,
    redirect: Url,
}

impl Endpoints {
    /// The same endpoints signed in as the OAuth client `client_id`.
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Result<Self, LoginError> {
        let client_id = client_id.into();
        if client_id.is_empty() || client_id.contains(char::is_whitespace) {
            return Err(LoginError::Malformed(UNNAMED));
        }
        self.client_id = client_id;
        Ok(self)
    }

    /// The same endpoints with the browser sent to `url` to approve a
    /// sign-in.
    pub fn with_authorize(mut self, url: &str) -> Result<Self, LoginError> {
        self.authorize = checked(AUTHORIZE, url)?;
        Ok(self)
    }

    /// The same endpoints with codes exchanged and tokens renewed at `url`.
    pub fn with_token(mut self, url: &str) -> Result<Self, LoginError> {
        self.token = checked(TOKEN, url)?;
        Ok(self)
    }

    /// The same endpoints with `url` as the page that shows the code to
    /// paste.
    pub fn with_redirect(mut self, url: &str) -> Result<Self, LoginError> {
        self.redirect = checked(REDIRECT, url)?;
        Ok(self)
    }

    /// The OAuth client a sign-in runs as.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Where the browser goes to approve a sign-in.
    pub fn authorize(&self) -> &Url {
        &self.authorize
    }

    /// Where a code is exchanged and tokens are renewed.
    pub fn token(&self) -> &Url {
        &self.token
    }

    /// The page that shows the code to paste.
    pub fn redirect(&self) -> &Url {
        &self.redirect
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        let parsed = |url: &str| Url::parse(url).expect("a built-in Claude URL parses");
        Self {
            client_id: CLIENT_ID.to_string(),
            authorize: parsed(AUTHORIZE_URL),
            token: parsed(TOKEN_URL),
            redirect: parsed(REDIRECT_URL),
        }
    }
}

fn checked(name: &'static str, url: &str) -> Result<Url, LoginError> {
    TargetPolicy::ExceptMetadata
        .validate(url.trim())
        .map_err(|error| LoginError::Endpoint {
            name,
            reason: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_endpoints_are_the_ones_the_claude_cli_uses() {
        let endpoints = Endpoints::default();

        assert_eq!(endpoints.client_id(), CLIENT_ID);
        assert_eq!(endpoints.authorize().as_str(), AUTHORIZE_URL);
        assert_eq!(endpoints.token().as_str(), TOKEN_URL);
        assert_eq!(endpoints.redirect().as_str(), REDIRECT_URL);
    }

    #[test]
    fn each_endpoint_keeps_a_loopback_or_lan_url_it_checked() {
        for url in [
            "https://platform.claude.com/v1/oauth/token",
            "http://127.0.0.1:4010/v1/oauth/token",
            "http://localhost:4010/v1/oauth/token",
            "http://10.0.0.5/v1/oauth/token",
        ] {
            let endpoints = Endpoints::default()
                .with_authorize(url)
                .and_then(|endpoints| endpoints.with_token(url))
                .and_then(|endpoints| endpoints.with_redirect(url))
                .unwrap_or_else(|error| panic!("{url}: {error}"));

            assert_eq!(endpoints.authorize().as_str(), url);
            assert_eq!(endpoints.token().as_str(), url);
            assert_eq!(endpoints.redirect().as_str(), url);
        }
    }

    #[test]
    fn an_endpoint_refuses_other_schemes_userinfo_a_missing_host_and_metadata() {
        type Builder = fn(Endpoints, &str) -> Result<Endpoints, LoginError>;
        let builders: [(&str, Builder); 3] = [
            (AUTHORIZE, Endpoints::with_authorize),
            (TOKEN, Endpoints::with_token),
            (REDIRECT, Endpoints::with_redirect),
        ];
        for (endpoint, build) in builders {
            for url in [
                "ftp://platform.claude.com/v1/oauth/token",
                "file:///etc/passwd",
                "https://user:secret@platform.claude.com/v1/oauth/token",
                "https://user@platform.claude.com/v1/oauth/token",
                "https://",
                "/v1/oauth/token",
                "http://169.254.169.254/latest",
                "http://[fd00:ec2::254]/v1",
            ] {
                match build(Endpoints::default(), url) {
                    Err(LoginError::Endpoint { name, reason }) => {
                        assert_eq!(name, endpoint, "{url}");
                        assert!(!reason.contains("secret"), "{url}: {reason}");
                    }
                    other => panic!("{endpoint} must refuse {url}, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_refused_endpoint_names_neither_its_url_nor_its_credentials() {
        let error = Endpoints::default()
            .with_token("https://someone:hunter2@platform.claude.com/v1/oauth/token")
            .expect_err("userinfo is refused");
        let shown = format!("{error} {error:?}");

        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(!shown.contains("platform.claude.com"), "{shown}");
    }

    #[test]
    fn a_client_id_must_be_one_word() {
        assert_eq!(
            Endpoints::default()
                .with_client_id("another-client")
                .expect("a client id")
                .client_id(),
            "another-client"
        );
        for client_id in ["", "two words", "trailing\n"] {
            assert!(
                matches!(
                    Endpoints::default().with_client_id(client_id),
                    Err(LoginError::Malformed(UNNAMED))
                ),
                "{client_id:?}"
            );
        }
    }
}
