use serde::Deserialize;
use serde::Serialize;

/// Which part of a person a crop frames, as its
/// [`KIND_SUFFIX`](crate::dataset::KIND_SUFFIX) companion names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FrameKind {
    /// The whole person, in a bucket of the photo's aspect.
    Body,
    /// A head and shoulders square.
    Head,
    /// A square on one hand, when the caption names hands.
    Hand,
}

impl FrameKind {
    /// The name the companion file holds: `body`, `head` or `hand`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Head => "head",
            Self::Hand => "hand",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_written_and_serialized_under_one_name() {
        for (kind, name) in [
            (FrameKind::Body, "body"),
            (FrameKind::Head, "head"),
            (FrameKind::Hand, "hand"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<FrameKind>(serde_json::json!(name)).unwrap(),
                kind
            );
        }
    }
}
