use crate::login::claude::Endpoints;
use crate::login::claude::Flow;

/// The only host a loopback redirect names, as the claude CLI's own sign-in
/// names it.
pub const LOOPBACK_HOST: &str = "localhost";

/// The scheme of every loopback redirect.
pub const LOOPBACK_SCHEME: &str = "http";

/// The path of every loopback redirect, and so the one path a loopback
/// listener has to serve.
pub const CALLBACK_PATH: &str = "/callback";

/// Where claude.com sends the browser once someone approves a sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Redirect {
    /// claude.com's own page, [`Endpoints::redirect`], which shows the code
    /// to paste.
    Paste,
    /// The caller's loopback listener, at `http://localhost:<port>/callback`.
    Loopback(u16),
}

impl Redirect {
    /// The `redirect_uri` the authorize link carries, which the exchange has
    /// to send again.
    pub fn uri(self, endpoints: &Endpoints) -> String {
        match self {
            Self::Paste => endpoints.redirect().to_string(),
            Self::Loopback(port) => {
                format!("{LOOPBACK_SCHEME}://{LOOPBACK_HOST}:{port}{CALLBACK_PATH}")
            }
        }
    }

    /// How the code gets back through this redirect.
    pub fn flow(self) -> Flow {
        match self {
            Self::Paste => Flow::Paste,
            Self::Loopback(_) => Flow::Loopback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::claude::REDIRECT_URL;

    #[test]
    fn a_loopback_redirect_is_the_one_the_claude_cli_sends_to_its_own_listener() {
        assert_eq!(
            Redirect::Loopback(54_545).uri(&Endpoints::default()),
            "http://localhost:54545/callback"
        );
        assert_eq!(Redirect::Loopback(54_545).flow(), Flow::Loopback);
    }

    #[test]
    fn a_paste_redirect_is_the_page_the_endpoints_name() {
        assert_eq!(Redirect::Paste.uri(&Endpoints::default()), REDIRECT_URL);
        assert_eq!(Redirect::Paste.flow(), Flow::Paste);
        let endpoints = Endpoints::default()
            .with_redirect("http://127.0.0.1:4010/oauth/code/callback")
            .expect("a loopback redirect");
        assert_eq!(
            Redirect::Paste.uri(&endpoints),
            "http://127.0.0.1:4010/oauth/code/callback"
        );
    }
}
