use serde::Deserialize;
use serde::Serialize;

/// A GitHub App's own identifier, the issuer of every JWT it signs.
///
/// It and [`crate::pull_request::InstallationId`] are both bare integers on
/// the wire and sit side by side in the same configuration, so each has its
/// own type: swapping them is a compile error rather than a `401`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApplicationId(u64);

impl ApplicationId {
    /// The App GitHub numbered `value`.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The number GitHub gave the App.
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::InstallationId;

    #[test]
    fn identifiers_are_bare_integers_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&ApplicationId::new(12345)).unwrap(),
            "12345"
        );
        assert_eq!(
            serde_json::from_str::<InstallationId>("67890").unwrap(),
            InstallationId::new(67890)
        );
        assert_eq!(
            serde_json::from_str::<ApplicationId>("12345").unwrap(),
            ApplicationId::new(12345)
        );
        assert_eq!(
            serde_json::to_string(&InstallationId::new(67890)).unwrap(),
            "67890"
        );
    }
}
