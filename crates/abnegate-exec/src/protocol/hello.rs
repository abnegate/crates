use serde::Deserialize;
use serde::Serialize;

/// Opens a connection: the protocol version a client speaks and the
/// capabilities it knows about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Hello {
    /// The protocol version the client speaks, such as
    /// [`PROTOCOL_VERSION`](super::PROTOCOL_VERSION)
    pub protocol_version: String,
    /// The capabilities the client knows about, by their
    /// [handshake names](super::Capability::as_str)
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Hello {
    /// Speak `protocol_version`, knowing about no capability.
    pub fn new(protocol_version: impl Into<String>) -> Self {
        Self {
            protocol_version: protocol_version.into(),
            capabilities: Vec::new(),
        }
    }

    /// Name the capabilities the client knows about.
    pub fn with_capabilities<I, S>(mut self, capabilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }
}
