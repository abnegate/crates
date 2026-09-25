use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

const STREAMABLE_HTTP: &str = "streamable-http";

/// How the CLI talks to an MCP server.
///
/// Read from its name, `streamable-http` included, which the MCP
/// specification calls [`McpTransport::Http`]; any other name reads as
/// [`McpTransport::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum McpTransport {
    /// A child process speaking over its stdin and stdout.
    Stdio,
    /// Streamable HTTP.
    Http,
    /// Server-sent events.
    Sse,
    /// A transport this crate does not attach a server over, such as Claude
    /// Code's `ws`: a server that names one is read, and never
    /// [valid](crate::McpServer::valid).
    Unsupported,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
            Self::Unsupported => "unsupported",
        }
    }

    /// The transport `name` names on the wire.
    fn named(name: &str) -> Self {
        match name {
            "stdio" => Self::Stdio,
            "http" | STREAMABLE_HTTP => Self::Http,
            "sse" => Self::Sse,
            _ => Self::Unsupported,
        }
    }

    /// Whether this transport reaches its server by URL rather than by
    /// starting a command.
    pub fn remote(self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }
}

impl fmt::Display for McpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for McpTransport {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = Cow::<'de, str>::deserialize(deserializer)?;
        Ok(Self::named(&name))
    }
}

#[cfg(test)]
mod tests {
    use super::McpTransport;

    #[test]
    fn a_transport_reads_and_writes_its_lowercase_name() {
        for (transport, name) in [
            (McpTransport::Stdio, "stdio"),
            (McpTransport::Http, "http"),
            (McpTransport::Sse, "sse"),
        ] {
            assert_eq!(transport.to_string(), name);
            assert_eq!(
                serde_json::to_value(transport).expect("serialisable"),
                serde_json::json!(name)
            );
            assert_eq!(
                serde_json::from_value::<McpTransport>(serde_json::json!(name))
                    .expect("deserialisable"),
                transport
            );
        }
    }

    #[test]
    fn only_http_and_sse_are_remote() {
        assert!(!McpTransport::Stdio.remote());
        assert!(McpTransport::Http.remote());
        assert!(McpTransport::Sse.remote());
        assert!(!McpTransport::Unsupported.remote());
    }

    /// Server documentation, and Claude Code, name streamable HTTP by the
    /// specification's name; anything else a client names reads as a
    /// transport this crate does not attach over, not as an error that
    /// would sink every other server beside it.
    #[test]
    fn the_specifications_name_reads_as_http_and_any_other_as_unsupported() {
        for (name, transport) in [
            ("streamable-http", McpTransport::Http),
            ("ws", McpTransport::Unsupported),
            ("grpc", McpTransport::Unsupported),
            ("HTTP", McpTransport::Unsupported),
        ] {
            assert_eq!(
                serde_json::from_value::<McpTransport>(serde_json::json!(name))
                    .expect("deserialisable"),
                transport,
                "{name}"
            );
        }
        assert!(serde_json::from_value::<McpTransport>(serde_json::json!(1)).is_err());
    }
}
