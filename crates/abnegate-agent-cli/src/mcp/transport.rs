use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

const STDIO: &str = "stdio";
const HTTP: &str = "http";
const SSE: &str = "sse";
const STREAMABLE_HTTP: &str = "streamable-http";

/// How the CLI talks to an MCP server.
///
/// Read from its name, `streamable-http` included, which the MCP
/// specification calls [`McpTransport::Http`] and which is written back as
/// `http`; any other name reads as [`McpTransport::Unsupported`], written
/// back under the name it was read by.
///
/// Only reading a name makes an [`McpTransport::Unsupported`], so one never
/// holds a name this crate attaches over, which would read back as another
/// transport:
///
/// ```compile_fail,E0603
/// use abnegate_agent_cli::McpTransport;
///
/// let transport = McpTransport::Unsupported("http".to_string());
/// # let _ = transport;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// A child process speaking over its stdin and stdout.
    Stdio,
    /// Streamable HTTP.
    Http,
    /// Server-sent events.
    Sse,
    /// A transport this crate does not attach a server over, such as Claude
    /// Code's `ws`, by the name the configuration gave it: a server that
    /// names one is read, written back as it was, and never
    /// [valid](crate::McpServer::valid).
    #[non_exhaustive]
    Unsupported(String),
}

impl McpTransport {
    /// The transport's name on the wire.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stdio => STDIO,
            Self::Http => HTTP,
            Self::Sse => SSE,
            Self::Unsupported(name) => name,
        }
    }

    /// The transport `name` names on the wire.
    fn named(name: &str) -> Self {
        match name {
            STDIO => Self::Stdio,
            HTTP | STREAMABLE_HTTP => Self::Http,
            SSE => Self::Sse,
            _ => Self::Unsupported(name.to_string()),
        }
    }

    /// Whether this transport reaches its server by URL rather than by
    /// starting a command.
    pub fn remote(&self) -> bool {
        matches!(self, Self::Http | Self::Sse)
    }
}

impl fmt::Display for McpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for McpTransport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
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
                serde_json::to_value(&transport).expect("serialisable"),
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
        assert!(!McpTransport::Unsupported("ws".to_string()).remote());
    }

    /// Server documentation, and Claude Code, name streamable HTTP by the
    /// specification's name; anything else a client names reads as a
    /// transport this crate does not attach over, not as an error that
    /// would sink every other server beside it.
    #[test]
    fn the_specifications_name_reads_as_http_and_any_other_as_unsupported() {
        for (name, transport) in [
            ("streamable-http", McpTransport::Http),
            ("ws", McpTransport::Unsupported("ws".to_string())),
            ("grpc", McpTransport::Unsupported("grpc".to_string())),
            ("HTTP", McpTransport::Unsupported("HTTP".to_string())),
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

    /// A transport read under a name this crate does not attach over is
    /// written back under that name, never under one of its own.
    #[test]
    fn an_unsupported_transport_is_written_back_under_the_name_it_was_read_by() {
        for name in ["ws", "grpc", "HTTP", "unsupported"] {
            let transport: McpTransport =
                serde_json::from_value(serde_json::json!(name)).expect("deserialisable");

            assert_eq!(transport.as_str(), name);
            assert_eq!(transport.to_string(), name);
            assert_eq!(
                serde_json::to_value(&transport).expect("serialisable"),
                serde_json::json!(name)
            );
        }
    }
}
