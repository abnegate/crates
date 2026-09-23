//! What this host can honour, as advertised during the handshake.

use crate::protocol::Capability;
use crate::protocol::OutboundMessage;
use crate::protocol::PROTOCOL_VERSION;

use super::confinement::Backend;
use super::confinement::Confinement;
use super::confinement::HOST_BACKEND;

impl Capability {
    /// The capabilities this host can honour.
    ///
    /// Each confinement capability is advertised only where the host's
    /// backend enforces what it claims: see [`Backend::enforces_execute_roots`]
    /// and [`Backend::enforces_single_process`].
    pub fn supported() -> Vec<String> {
        let backend = HOST_BACKEND.filter(|_| Confinement::is_available());
        Self::ALL
            .iter()
            .filter(|capability| capability.is_honoured_by(backend))
            .map(|capability| capability.as_str().to_string())
            .collect()
    }

    fn is_honoured_by(&self, backend: Option<Backend>) -> bool {
        match self {
            Capability::Confinement => backend.is_some(),
            Capability::ConfinementProcessTree => {
                backend.is_some_and(Backend::enforces_execute_roots)
            }
            Capability::ConfinementSingleProcess => {
                backend.is_some_and(Backend::enforces_single_process)
            }
            Capability::Cancel
            | Capability::Stdin
            | Capability::Logs
            | Capability::ProcessGroup => true,
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
    fn bubblewrap_advertises_only_the_confinement_it_enforces() {
        let bubblewrap = Some(Backend::Bubblewrap);

        assert!(Capability::Confinement.is_honoured_by(bubblewrap));
        assert!(!Capability::ConfinementProcessTree.is_honoured_by(bubblewrap));
        assert!(!Capability::ConfinementSingleProcess.is_honoured_by(bubblewrap));
    }

    #[test]
    fn seatbelt_advertises_every_confinement_capability() {
        let seatbelt = Some(Backend::Seatbelt);

        assert!(Capability::Confinement.is_honoured_by(seatbelt));
        assert!(Capability::ConfinementProcessTree.is_honoured_by(seatbelt));
        assert!(Capability::ConfinementSingleProcess.is_honoured_by(seatbelt));
    }

    #[test]
    fn a_host_without_a_backend_advertises_no_confinement() {
        for capability in [
            Capability::Confinement,
            Capability::ConfinementProcessTree,
            Capability::ConfinementSingleProcess,
        ] {
            assert!(!capability.is_honoured_by(None), "{capability:?}");
        }
        assert!(Capability::Cancel.is_honoured_by(None));
    }

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
