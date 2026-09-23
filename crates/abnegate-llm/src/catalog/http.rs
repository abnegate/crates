use crate::catalog::error::CatalogError;
use reqwest::Client;
use reqwest::Proxy;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const POOL_MAX_IDLE_PER_HOST: usize = 10;
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const USER_AGENT: &str = concat!("abnegate-llm/", env!("CARGO_PKG_VERSION"));

pub(crate) fn build_client(proxy_url: Option<&str>) -> Result<Client, CatalogError> {
    let mut builder = Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .user_agent(USER_AGENT);

    if let Some(proxy_url) = proxy_url {
        builder = builder.proxy(Proxy::all(proxy_url)?);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_accepts_a_proxy() {
        assert!(build_client(None).is_ok());
        assert!(build_client(Some("http://127.0.0.1:3128")).is_ok());
    }

    #[test]
    fn build_client_rejects_a_malformed_proxy() {
        assert!(build_client(Some("not a url")).is_err());
    }
}
