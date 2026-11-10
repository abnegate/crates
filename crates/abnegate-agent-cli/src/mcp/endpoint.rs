use std::collections::BTreeMap;
use std::fmt;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;

const SCHEME_SEPARATOR: &str = "://";
const USERINFO_SEPARATOR: char = '@';
const AUTHORITY_END: [char; 4] = ['/', '\\', '?', '#'];

/// Where a remote [`McpServer`](crate::McpServer) is reached, with every
/// reference in its URL and headers resolved: what
/// [`McpServer::endpoint`](crate::McpServer::endpoint) and
/// [`McpServer::endpoint_with`](crate::McpServer::endpoint_with) give.
///
/// `Debug` never prints a header value, nor anything of the URL but its
/// scheme, host and port, since its user-info, path, query and fragment may
/// each carry a credential.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpEndpoint {
    /// The resolved URL the server is reached at.
    ///
    /// It holds every secret its references resolved to, so it must never be
    /// logged or shown: print the endpoint's `Debug`, which redacts it,
    /// instead.
    pub url: String,
    /// Each header the server is sent, by name, with its resolved value.
    pub headers: BTreeMap<String, SecretValue>,
}

impl fmt::Debug for McpEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpEndpoint")
            .field("url", &shown(&self.url))
            .field("headers", &self.headers)
            .finish()
    }
}

/// `url`'s scheme, host and port, with its user-info replaced by
/// [`REDACTED`], and everything after its authority by one [`REDACTED`]
/// behind the delimiter that starts it.
fn shown(url: &str) -> String {
    let (scheme, rest) = url.split_once(SCHEME_SEPARATOR).unwrap_or(("", url));
    let authority_length = rest.find(AUTHORITY_END).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_length);
    let host = authority.rsplit_once(USERINFO_SEPARATOR).map_or_else(
        || authority.to_string(),
        |(_, host)| format!("{REDACTED}{USERINFO_SEPARATOR}{host}"),
    );
    let mut shown = String::with_capacity(url.len());
    if url.contains(SCHEME_SEPARATOR) {
        shown.push_str(scheme);
        shown.push_str(SCHEME_SEPARATOR);
    }
    shown.push_str(&host);
    if let Some(delimiter) = rest.chars().next() {
        shown.push(delimiter);
        shown.push_str(REDACTED);
    }
    redact(&shown).into_owned()
}

#[cfg(test)]
mod tests {
    use super::shown;

    #[test]
    fn a_url_is_shown_as_its_scheme_host_and_port_alone() {
        for (url, expected) in [
            ("https://example.com", "https://example.com"),
            ("https://example.com/mcp", "https://example.com/[REDACTED]"),
            (
                "https://example.com:8443/abc123/mcp",
                "https://example.com:8443/[REDACTED]",
            ),
            (
                "https://user:pass@example.com/mcp",
                "https://[REDACTED]@example.com/[REDACTED]",
            ),
            (
                "https://example.com/mcp?key=abc123",
                "https://example.com/[REDACTED]",
            ),
            (
                "https://example.com#abc123",
                "https://example.com#[REDACTED]",
            ),
            ("https://a@b@example.com", "https://[REDACTED]@example.com"),
            (
                "https://example.com?key=x",
                "https://example.com?[REDACTED]",
            ),
            ("example.com/mcp", "example.com/[REDACTED]"),
        ] {
            assert_eq!(shown(url), expected, "{url}");
        }
    }
}
