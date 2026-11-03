use crate::address::literal;
use crate::address::must_not_be_fetched;
use crate::error::Error;
use crate::error::Result;
use crate::metadata;
use reqwest::Url;

const SCHEMES: [&str; 2] = ["http", "https"];
const INTERNAL_NAME: &str = "localhost";
const INTERNAL_SUFFIXES: [&str; 3] = [".localhost", ".local", ".internal"];

/// Parse `raw` and reject schemes, hosts, and addresses that must not be
/// fetched on behalf of a caller, the cloud metadata targets
/// [`metadata::is_url`] names among them, even where their address is public.
///
/// This reads text. The address a name resolves to is chosen afterwards, so a
/// fetch also needs [`PublicClient`](crate::PublicClient), which refuses the
/// resolved addresses as well.
pub fn validate_public_url(raw: &str) -> Result<Url> {
    let url = parse(raw)?;
    let host = url.host_str().ok_or(Error::MissingHost)?;

    match literal(host) {
        Some(ip) if must_not_be_fetched(ip) => return Err(Error::PrivateAddress),
        Some(_) => {}
        None if is_internal_name(host) => return Err(Error::InternalHost),
        None => {}
    }
    if metadata::is_url(&url) {
        return Err(Error::MetadataTarget);
    }
    Ok(url)
}

fn is_internal_name(host: &str) -> bool {
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    name == INTERNAL_NAME
        || INTERNAL_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Parse `raw` and reject what no fetch may carry, whatever its target: a
/// scheme other than `http` or `https`, embedded credentials, or no host.
pub(crate) fn parse(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| Error::InvalidUrl)?;
    if !SCHEMES.contains(&url.scheme()) {
        return Err(Error::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::EmbeddedCredentials);
    }
    if url.host_str().is_none() {
        return Err(Error::MissingHost);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_non_global_ranges_are_refused_as_urls() {
        for raw in [
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "240.0.0.1",
            "[64:ff9b::7f00:1]",
            "[2002:7f00:1::]",
            "[fec0::1]",
        ] {
            assert!(
                matches!(
                    validate_public_url(&format!("http://{raw}/")),
                    Err(Error::PrivateAddress)
                ),
                "{raw} passed the URL check"
            );
        }
    }

    #[test]
    fn rejects_private_and_internal_targets() {
        for url in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://localhost/admin",
            "http://169.254.169.254/latest",
            "http://metadata.google.internal/",
            "http://printer.local/",
            "file:///etc/passwd",
            "https://user:pass@example.com/",
        ] {
            assert!(validate_public_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn cloud_metadata_targets_in_public_ranges_are_refused() {
        for url in [
            "http://168.63.129.16/machine",
            "http://[::ffff:168.63.129.16]/machine",
            "http://metadata.goog/computeMetadata/v1",
            "http://METADATA.GOOG./computeMetadata/v1",
        ] {
            assert!(
                matches!(validate_public_url(url), Err(Error::MetadataTarget)),
                "{url} passed the URL check"
            );
        }
        for url in ["http://168.63.129.17/", "https://metadata.example/"] {
            assert!(validate_public_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_every_spelling_of_loopback() {
        for url in [
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://localhost./",
            "http://LOCALHOST/",
            "http://127.0.0.1./",
        ] {
            assert!(validate_public_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn accepts_public_https() {
        assert_eq!(
            validate_public_url("https://example.com/docs")
                .expect("a public URL")
                .as_str(),
            "https://example.com/docs"
        );
    }

    #[test]
    fn names_which_merely_resemble_internal_ones_are_accepted() {
        for url in [
            "https://notlocalhost.example/",
            "https://internal.example/",
            "https://local.example/",
        ] {
            assert!(validate_public_url(url).is_ok(), "{url}");
        }
    }
}
