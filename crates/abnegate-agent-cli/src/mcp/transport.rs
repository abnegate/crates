use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// How the CLI talks to an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum McpTransport {
    /// A child process speaking over its stdin and stdout.
    Stdio,
    /// Streamable HTTP.
    Http,
    /// Server-sent events.
    Sse,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
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
    }
}
