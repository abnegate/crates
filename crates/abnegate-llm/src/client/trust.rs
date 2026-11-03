/// Who chose an [`LlmConfig::base_url`](crate::LlmConfig::base_url), and so
/// how far the client trusts the endpoint behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Trust {
    /// The operator, in the deployment's own configuration. The endpoint may
    /// be any host, including loopback, the LAN or a compose service name, and
    /// is reached through the process's proxy settings. A refusal reaches the
    /// caller as the endpoint sent it, without the key the client sent.
    #[default]
    Operator,
    /// A tenant, in settings saved through an API. The endpoint is never sent
    /// to a link-local or cloud metadata address, whether the URL names one or
    /// its host resolves to one, and is never reached through a proxy. A
    /// refusal is reported only as its status and the endpoint's own error
    /// message, capped and in OpenAI's error envelope, and an error sent in
    /// place of a streamed chunk only as that message, capped, so neither
    /// reflects back whatever else a host answers. A successful answer that
    /// does not parse is still reported with the parser's own error, which
    /// may quote a fragment of it.
    Tenant,
}
