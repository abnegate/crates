use std::cmp::Reverse;
use std::sync::Arc;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use reqwest::Url;

use super::McpEndpoint;
use super::McpServer;

/// The shortest value scrubbed from a failure's text: one shorter would
/// replace digits and words it merely shares, such as a status code.
const MINIMUM_SCRUBBED_LENGTH: usize = 6;

/// What marks a configured value as one a bound secret may expand into.
const REFERENCE: &str = "${";

/// Any URL, for [`encodings`] to percent-encode a value into each of its
/// parts.
const BASE: &str = "http://host/";

/// What every text about one remote server is scrubbed of before it is
/// shown or logged: each value its secrets put on the wire, as written and
/// as a URL percent-encodes it.
#[derive(Clone, Default)]
pub(super) struct Scrubber {
    secrets: Arc<[SecretValue]>,
}

impl Scrubber {
    /// Every secret bound to `server`, and each value of `endpoint` its
    /// configuration builds from a reference, a header's or the URL, that is
    /// at least [`MINIMUM_SCRUBBED_LENGTH`] long, with each form a URL
    /// encodes it in, longest first, so a value holding another is replaced
    /// whole.
    pub(super) fn new(server: &McpServer, endpoint: &McpEndpoint) -> Self {
        let referring = |raw: Option<&str>| raw.is_some_and(|raw| raw.contains(REFERENCE));
        let expanded = endpoint
            .headers
            .iter()
            .filter(|(name, _)| referring(server.headers.get(*name).map(SecretValue::expose)))
            .map(|(_, value)| value.expose().to_string());
        let url = referring(server.url.as_deref()).then(|| endpoint.url.clone());
        let mut secrets: Vec<String> = server
            .secrets
            .values()
            .map(|secret| secret.expose().to_string())
            .chain(expanded)
            .chain(url)
            .filter(|secret| secret.len() >= MINIMUM_SCRUBBED_LENGTH)
            .flat_map(|secret| {
                let encoded = encodings(&secret);
                std::iter::once(secret).chain(encoded)
            })
            .collect();
        secrets.sort_by_key(|secret| Reverse(secret.len()));
        secrets.dedup();
        Self {
            secrets: secrets.into_iter().map(SecretValue::new).collect(),
        }
    }

    /// `text` with each secret replaced by [`REDACTED`], and anything else
    /// credential-shaped redacted too.
    pub(super) fn scrubbed(&self, text: &str) -> String {
        let replaced = self.secrets.iter().fold(text.to_string(), |text, secret| {
            text.replace(secret.expose(), REDACTED)
        });
        redact(&replaced).into_owned()
    }
}

/// `value` as a URL percent-encodes it in its user-info, path, query and
/// fragment, wherever that differs from `value` itself.
fn encodings(value: &str) -> Vec<String> {
    let Ok(base) = Url::parse(BASE) else {
        return Vec::new();
    };
    let mut url = base.clone();
    let mut encoded = Vec::new();
    if url.set_username(value).is_ok() {
        encoded.push(url.username().to_string());
    }
    url = base.clone();
    url.set_path(value);
    encoded.push(url.path().trim_start_matches('/').to_string());
    url = base.clone();
    url.set_query(Some(value));
    encoded.extend(url.query().map(str::to_string));
    url = base;
    url.set_fragment(Some(value));
    encoded.extend(url.fragment().map(str::to_string));
    encoded.retain(|form| form != value);
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_scrubbed_as_written_and_as_a_url_encodes_it() {
        let server = McpServer::remote("https://host/${KEY}/mcp?key=${KEY}")
            .with_header("X-Key", "${KEY}")
            .with_secret("KEY", "open sesame/\"9c1e\"");
        let endpoint = server.endpoint().expect("every reference is bound");
        let scrubber = Scrubber::new(&server, &endpoint);

        for form in [
            "open sesame/\"9c1e\"",
            "open%20sesame/%229c1e%22",
            "open%20sesame%2F%229c1e%22",
        ] {
            let scrubbed = scrubber.scrubbed(&format!("refused {form} here"));
            assert_eq!(scrubbed, format!("refused {REDACTED} here"), "{form}");
        }
    }

    #[test]
    fn a_value_shorter_than_the_minimum_is_left_alone() {
        let server = McpServer::remote("https://host/mcp")
            .with_header("X-Version", "${VERSION}")
            .with_secret("VERSION", "1");
        let endpoint = server.endpoint().expect("every reference is bound");

        assert_eq!(
            Scrubber::new(&server, &endpoint).scrubbed("HTTP 401"),
            "HTTP 401"
        );
    }
}
