use reqwest::Url;

use crate::provider::ProviderError;

/// `path` under `base_url`, refusing a base that does not parse or is not `http` or `https`. The checked URL is handed back, so only a checked URL is ever requested.
pub(crate) fn endpoint(base_url: &str, path: &str) -> Result<Url, ProviderError> {
    let joined = format!("{}{path}", base_url.trim_end_matches('/'));
    let url = Url::parse(&joined)
        .map_err(|error| ProviderError::config(format!("invalid base URL {base_url}: {error}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        scheme => Err(ProviderError::config(format!(
            "invalid base URL {base_url}: the {scheme} scheme is not http or https"
        ))),
    }
}
