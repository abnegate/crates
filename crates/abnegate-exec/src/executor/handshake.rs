//! What this host can honour, as advertised during the handshake.

use crate::protocol::Capability;
use crate::protocol::OutboundMessage;
use crate::protocol::PROTOCOL_VERSION;

use super::confinement::Confinement;

impl Capability {
    /// The capabilities this host can honour.
    pub fn supported() -> Vec<String> {
        Self::ALL
            .iter()
            .filter(|capability| capability.is_supported())
            .map(|capability| capability.as_str().to_string())
            .collect()
    }

    fn is_supported(&self) -> bool {
        match self {
            Capability::Confinement | Capability::ConfinementProcessTree => {
                Confinement::is_available()
            }
            _ => true,
        }
    }
}

impl OutboundMessage {
    /// Create a HelloAck with the capabilities this host can honour
    pub fn hello_ack() -> Self {
        OutboundMessage::HelloAck {
            protocol_version: PROTOCOL_VERSION.to_string(),
            runner_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: Capability::supported(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_supported_tracks_confinement_availability() {
        let supported = Capability::supported();
        assert!(supported.contains(&"cancel".to_string()));
        assert_eq!(
            supported.contains(&"confinement".to_string()),
            Confinement::is_available()
        );
    }
}
