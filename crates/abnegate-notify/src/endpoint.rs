//! A webhook target that has been checked before anything is sent to it.

mod error;

use std::fmt;

use abnegate_secret::SecretValue;
use reqwest::Client;
use reqwest::RequestBuilder;
use url::Host;
use url::Url;

pub use crate::endpoint::error::EndpointError;

const LOOPBACK_SUFFIX: &str = ".localhost";
const LOOPBACK_NAME: &str = "localhost";
const REQUIRED_SCHEME: &str = "https";

/// A validated webhook URL, held as the credential it is.
///
/// An outbound webhook is a server-side request to an address someone else
/// chose, so the URL is checked once here rather than at each call site. The
/// host allowlist is the load-bearing control: because each backend talks to
/// exactly one provider, an exact-match allowlist removes the whole class of
/// redirect, rebinding and encoded-address bypasses that a private-range
/// blocklist has to chase. The scheme, authority and literal-address rules
/// keep the type sound on its own, for an allowlist wider than one host.
#[derive(Clone)]
pub struct Endpoint {
    url: SecretValue,
    host: String,
}

impl Endpoint {
    /// Validate `url` and accept it only if its host is one of `allowed`.
    ///
    /// `allowed` is matched exactly against the parsed host, which `url` has
    /// already lowercased and punycode-encoded, so a lookalike host cannot
    /// match by casing or by Unicode confusable. A provider serves its hooks
    /// on the default port, so naming any other one is refused. What is kept
    /// is the parsed form, so the URL requested is the one that was checked.
    pub fn new(url: &str, allowed: &[&str]) -> Result<Self, EndpointError> {
        let parsed = Url::parse(url).map_err(|_| EndpointError::Malformed)?;

        if parsed.scheme() != REQUIRED_SCHEME {
            return Err(EndpointError::Scheme {
                scheme: parsed.scheme().to_string(),
            });
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(EndpointError::EmbeddedCredentials);
        }

        if let Some(port) = parsed.port() {
            return Err(EndpointError::Port { port });
        }

        let host = parsed.host().ok_or(EndpointError::MissingHost)?;
        let name = match host {
            Host::Domain(domain) => domain.to_string(),
            Host::Ipv4(address) => {
                return Err(EndpointError::AddressLiteral {
                    host: address.to_string(),
                });
            }
            Host::Ipv6(address) => {
                return Err(EndpointError::AddressLiteral {
                    host: address.to_string(),
                });
            }
        };

        if name == LOOPBACK_NAME || name.ends_with(LOOPBACK_SUFFIX) {
            return Err(EndpointError::Loopback { host: name });
        }

        if !allowed.contains(&name.as_str()) {
            return Err(EndpointError::HostNotAllowed { host: name });
        }

        Ok(Self {
            url: SecretValue::new(parsed.as_str()),
            host: name,
        })
    }

    /// The host, which is safe to log. The rest of the URL is not.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub(crate) fn post(&self, client: &Client) -> RequestBuilder {
        client.post(self.url.expose())
    }

    /// An endpoint pointed at a local test server, bypassing every check.
    #[cfg(test)]
    pub(crate) fn for_test(url: &str) -> Self {
        let host = Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .unwrap_or_else(|| "test".to_string());
        Self {
            url: SecretValue::new(url),
            host,
        }
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Endpoint")
            .field("host", &self.host)
            .field("url", &self.url)
            .finish()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: &[&str] = &["hooks.slack.com"];
    const SECRET_PATH: &str = "https://hooks.slack.com/services/T000/B000/xxxxSECRETxxxx";

    #[test]
    fn an_allowed_https_host_is_accepted() {
        let endpoint = Endpoint::new(SECRET_PATH, ALLOWED).expect("allowed host");
        assert_eq!(endpoint.host(), "hooks.slack.com");
    }

    #[test]
    fn the_url_never_appears_in_debug_or_display() {
        let endpoint = Endpoint::new(SECRET_PATH, ALLOWED).expect("allowed host");

        let debug = format!("{endpoint:?}");
        let display = endpoint.to_string();

        assert!(!debug.contains("xxxxSECRETxxxx"), "Debug leaked: {debug}");
        assert!(!debug.contains("/services/"), "Debug leaked: {debug}");
        assert!(!display.contains("xxxxSECRETxxxx"), "Display leaked");
        assert_eq!(display, "hooks.slack.com");
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn plain_http_is_refused() {
        let error = Endpoint::new("http://hooks.slack.com/services/T/B/x", ALLOWED)
            .expect_err("http must be refused");
        assert_eq!(
            error,
            EndpointError::Scheme {
                scheme: "http".to_string()
            }
        );
    }

    #[test]
    fn a_link_local_metadata_address_is_refused() {
        let error = Endpoint::new("https://169.254.169.254/latest/meta-data/", ALLOWED)
            .expect_err("an IP literal must be refused");
        assert_eq!(
            error,
            EndpointError::AddressLiteral {
                host: "169.254.169.254".to_string()
            }
        );
    }

    #[test]
    fn private_and_loopback_addresses_are_refused() {
        for url in [
            "https://127.0.0.1/hook",
            "https://10.0.0.1/hook",
            "https://192.168.1.1/hook",
            "https://[::1]/hook",
            "https://[fd00::1]/hook",
        ] {
            let error = Endpoint::new(url, ALLOWED).expect_err("must be refused");
            assert!(
                matches!(error, EndpointError::AddressLiteral { .. }),
                "{url} produced {error:?}"
            );
        }
    }

    #[test]
    fn loopback_by_name_is_refused_even_when_allowlisted() {
        for host in ["localhost", "anything.localhost"] {
            let url = format!("https://{host}/hook");
            let error = Endpoint::new(&url, &[host]).expect_err("must be refused");
            assert_eq!(
                error,
                EndpointError::Loopback {
                    host: host.to_string()
                }
            );
        }
    }

    #[test]
    fn a_host_outside_the_allowlist_is_refused() {
        let error =
            Endpoint::new("https://evil.example.com/hook", ALLOWED).expect_err("must be refused");
        assert_eq!(
            error,
            EndpointError::HostNotAllowed {
                host: "evil.example.com".to_string()
            }
        );
    }

    #[test]
    fn a_host_that_merely_ends_with_an_allowed_host_is_refused() {
        let error = Endpoint::new("https://evilhooks.slack.com.attacker.test/hook", ALLOWED)
            .expect_err("suffix matching must not apply");
        assert!(matches!(error, EndpointError::HostNotAllowed { .. }));

        let prefixed = Endpoint::new("https://nothooks.slack.com/hook", ALLOWED)
            .expect_err("prefix padding must not match");
        assert!(matches!(prefixed, EndpointError::HostNotAllowed { .. }));
    }

    #[test]
    fn credentials_in_the_authority_are_refused() {
        for url in [
            "https://user@hooks.slack.com/hook",
            "https://user:pass@hooks.slack.com/hook",
        ] {
            assert_eq!(
                Endpoint::new(url, ALLOWED).expect_err("must be refused"),
                EndpointError::EmbeddedCredentials
            );
        }
    }

    #[test]
    fn non_http_schemes_are_refused() {
        for url in [
            "file:///etc/passwd",
            "gopher://hooks.slack.com/",
            "ftp://hooks.slack.com/",
        ] {
            let error = Endpoint::new(url, ALLOWED).expect_err("must be refused");
            assert!(
                matches!(
                    error,
                    EndpointError::Scheme { .. } | EndpointError::Malformed
                ),
                "{url} produced {error:?}"
            );
        }
    }

    #[test]
    fn unparseable_input_is_refused_without_quoting_it() {
        let error = Endpoint::new("not a url at all", ALLOWED).expect_err("must be refused");
        assert_eq!(error, EndpointError::Malformed);
        assert!(!error.to_string().contains("not a url"));
    }

    #[test]
    fn host_casing_is_normalised_before_matching() {
        let endpoint =
            Endpoint::new("https://HOOKS.Slack.COM/services/T/B/x", ALLOWED).expect("same host");
        assert_eq!(endpoint.host(), "hooks.slack.com");
    }

    #[test]
    fn a_port_other_than_the_default_is_refused() {
        for (url, port) in [
            ("https://hooks.slack.com:8443/services/T/B/x", 8443),
            ("https://hooks.slack.com:80/services/T/B/x", 80),
        ] {
            assert_eq!(
                Endpoint::new(url, ALLOWED).expect_err("must be refused"),
                EndpointError::Port { port }
            );
        }
        assert!(Endpoint::new("https://hooks.slack.com:443/services/T/B/x", ALLOWED).is_ok());
    }

    #[test]
    fn the_parsed_url_is_what_is_kept() {
        let endpoint = Endpoint::new("  https://HOOKS.Slack.COM:443/services/T/../T/B/x", ALLOWED)
            .expect("same host");
        assert_eq!(
            endpoint.url.expose(),
            "https://hooks.slack.com/services/T/B/x"
        );
    }
}
