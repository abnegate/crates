use serde::Deserialize;
use serde::Serialize;

/// How a Claude sign-in's code gets back to the caller, spelled in
/// lowercase on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Flow {
    /// claude.com sends the browser to the caller's loopback listener with
    /// the code, and nothing is copied. It needs the browser on the machine
    /// the listener runs on.
    Loopback,
    /// claude.com shows the code, and someone pastes it back.
    Paste,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_flow_travels_in_lowercase() {
        for (flow, spelled) in [(Flow::Loopback, "loopback"), (Flow::Paste, "paste")] {
            assert_eq!(
                serde_json::to_value(flow).expect("serialise"),
                json!(spelled)
            );
            assert_eq!(
                serde_json::from_value::<Flow>(json!(spelled)).expect("deserialise"),
                flow
            );
        }
        assert!(serde_json::from_value::<Flow>(json!("device")).is_err());
    }
}
