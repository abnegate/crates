use std::fmt::Display;

use reqwest::Url;

use crate::provider::ProviderError;

const PATH_SEPARATOR: char = '/';

/// `path` under `base_url`, refusing a base that does not parse, cannot be a
/// base, or is not `http` or `https`.
///
/// Each segment of `path` is appended to the base's own path, after any
/// trailing `/`, so a base such as `http://host/ollama` keeps its prefix and
/// `http://gateway/v1?api-version=1` keeps its query; a fragment is dropped.
/// The checked URL is handed back, so only a checked URL is ever requested.
pub(crate) fn endpoint(base_url: &str, path: &str) -> Result<Url, ProviderError> {
    let invalid = |reason: &dyn Display| {
        ProviderError::config(format!("invalid base URL {base_url}: {reason}"))
    };
    let mut url = Url::parse(base_url).map_err(|error| invalid(&error))?;
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(invalid(&format!(
            "the {scheme} scheme is not http or https"
        )));
    }

    let prefix = url.path().trim_end_matches(PATH_SEPARATOR).to_string();
    url.set_path(&prefix);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|()| invalid(&"it cannot be a base"))?
        .pop_if_empty()
        .extend(
            path.split(PATH_SEPARATOR)
                .filter(|segment| !segment.is_empty()),
        );
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(base_url: &str, path: &str) -> String {
        endpoint(base_url, path).unwrap().to_string()
    }

    #[test]
    fn a_path_prefix_on_the_base_is_kept() {
        assert_eq!(
            joined("http://host/ollama", "/api/embeddings"),
            "http://host/ollama/api/embeddings"
        );
    }

    #[test]
    fn a_query_on_the_base_is_kept_after_the_path() {
        assert_eq!(
            joined("http://gw/v1?api-version=1", "/embeddings"),
            "http://gw/v1/embeddings?api-version=1"
        );
    }

    #[test]
    fn a_fragment_on_the_base_is_dropped() {
        assert_eq!(
            joined("http://gw/v1#section", "/embeddings"),
            "http://gw/v1/embeddings"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_is_ignored() {
        for base_url in ["http://host/v1/", "http://host/v1//"] {
            assert_eq!(
                joined(base_url, "/embeddings"),
                "http://host/v1/embeddings",
                "{base_url}"
            );
        }
        for base_url in ["http://host", "http://host/"] {
            assert_eq!(
                joined(base_url, "/api/embeddings"),
                "http://host/api/embeddings",
                "{base_url}"
            );
        }
    }

    #[test]
    fn a_base_that_is_not_http_or_does_not_parse_is_refused() {
        for base_url in ["ftp://host/v1", "mailto:someone@example.com", "garbage", ""] {
            let error = endpoint(base_url, "/embeddings").unwrap_err();

            assert!(
                matches!(error, ProviderError::Config { .. }),
                "{base_url}: {error:?}"
            );
        }
    }
}
