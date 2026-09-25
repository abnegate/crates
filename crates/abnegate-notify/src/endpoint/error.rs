use thiserror::Error;

use crate::endpoint::REQUIRED_SCHEME;

/// Why a webhook URL was refused.
///
/// A rejected URL is never quoted back: the path of a webhook URL is the
/// credential, so only the host reaches the message.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndpointError {
    /// The URL could not be parsed at all.
    #[error("the webhook URL could not be parsed")]
    Malformed,

    /// The URL uses a scheme other than `https`.
    #[error("a webhook must use {REQUIRED_SCHEME}, not {scheme}")]
    #[non_exhaustive]
    Scheme {
        /// The scheme the URL named.
        scheme: String,
    },

    /// The URL names no host.
    #[error("the webhook URL has no host")]
    MissingHost,

    /// The URL carries a user name or password before its host.
    #[error("the webhook URL embeds credentials in its authority")]
    EmbeddedCredentials,

    /// The URL names a port, which a provider's hooks never need.
    #[error("a webhook may not name port {port}")]
    #[non_exhaustive]
    Port {
        /// The port the URL named.
        port: u16,
    },

    /// The host is an IPv4 or IPv6 address rather than a name.
    #[error("{host} is an IP literal, which a webhook may not target")]
    #[non_exhaustive]
    AddressLiteral {
        /// The address, as the URL parser normalised it.
        host: String,
    },

    /// The host is `localhost` or a name under `.localhost`.
    #[error("{host} resolves to the local machine")]
    #[non_exhaustive]
    Loopback {
        /// The refused host.
        host: String,
    },

    /// The host is not one the channel's provider serves hooks from.
    #[error("{host} is not an allowed host for this channel")]
    #[non_exhaustive]
    HostNotAllowed {
        /// The refused host, lowercased and punycode-encoded.
        host: String,
    },
}
