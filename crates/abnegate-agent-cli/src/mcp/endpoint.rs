use std::collections::BTreeMap;
use std::fmt;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;

const SCHEME_SEPARATOR: &str = "://";
const USERINFO_SEPARATOR: char = '@';
const AUTHORITY_END: [char; 4] = ['/', '\\', '?', '#'];
const PATH_END: [char; 2] = ['?', '#'];

/// Where a remote [`McpServer`](crate::McpServer) is reached, with every
/// reference in its URL and headers resolved: what
/// [`McpServer::endpoint`](crate::McpServer::endpoint) and
/// [`McpServer::endpoint_with`](crate::McpServer::endpoint_with) give.
///
/// `Debug` never prints a header value, nor the URL's user-info, query or
/// fragment, any of which may carry a credential.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpEndpoint {
    /// The resolved URL the server is reached at.
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

/// `url` with its user-info, query and fragment each replaced by
/// [`REDACTED`], and anything credential-shaped left in it redacted too.
fn shown(url: &str) -> String {
    let (scheme, rest) = url.split_once(SCHEME_SEPARATOR).unwrap_or(("", url));
    let authority_length = rest.find(AUTHORITY_END).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_length);
    let host = authority.rsplit_once(USERINFO_SEPARATOR).map_or_else(
        || authority.to_string(),
        |(_, host)| format!("{REDACTED}{USERINFO_SEPARATOR}{host}"),
    );
    let path_length = rest.find(PATH_END).unwrap_or(rest.len());
    let (path, tail) = rest.split_at(path_length);
    let mut shown = String::with_capacity(url.len());
    if url.contains(SCHEME_SEPARATOR) {
        shown.push_str(scheme);
        shown.push_str(SCHEME_SEPARATOR);
    }
    shown.push_str(&host);
    shown.push_str(path);
    if let Some(delimiter) = tail.chars().next() {
        shown.push(delimiter);
        shown.push_str(REDACTED);
    }
    redact(&shown).into_owned()
}

#[cfg(test)]
mod tests {
    use super::shown;

    #[test]
    fn a_url_is_shown_without_its_userinfo_query_or_fragment() {
        for (url, expected) in [
            ("https://example.com/mcp", "https://example.com/mcp"),
            (
                "https://user:pass@example.com/mcp",
                "https://[REDACTED]@example.com/mcp",
            ),
            (
                "https://example.com/mcp?key=abc123",
                "https://example.com/mcp?[REDACTED]",
            ),
            (
                "https://example.com/mcp#abc123",
                "https://example.com/mcp#[REDACTED]",
            ),
            ("https://a@b@example.com", "https://[REDACTED]@example.com"),
            (
                "https://example.com?key=x",
                "https://example.com?[REDACTED]",
            ),
            ("example.com/mcp", "example.com/mcp"),
        ] {
            assert_eq!(shown(url), expected, "{url}");
        }
    }
}
