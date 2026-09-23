use crate::error::Result;
use crate::public::PublicClient;
use reqwest::Client;
use reqwest::Method;
use reqwest::RequestBuilder;

/// Where a [`ReqwestHttpClient`](crate::ReqwestHttpClient) may send requests.
#[derive(Debug, Clone)]
pub(super) enum Outbound {
    /// Anywhere: the caller chose every URL this client will be handed.
    Trusted(Client),
    /// Only where [`PublicClient`] allows, checked before every request.
    Public(PublicClient),
}

impl Outbound {
    pub(super) fn request(&self, method: Method, url: &str) -> Result<RequestBuilder> {
        match self {
            Self::Trusted(client) => Ok(client.request(method, url)),
            Self::Public(client) => client.request(method, url),
        }
    }
}
