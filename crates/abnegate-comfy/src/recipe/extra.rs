use serde::Deserialize;

/// Something a recipe's graph supports beyond its slots, declared in the
/// catalog's `extras` list so a host never infers it from a recipe id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RecipeExtra {
    /// The graph accepts an IP-Adapter face reference for the subject.
    Face,
    /// Every result gets a face-refine pass.
    Refine,
    /// A result gets a face-refine pass when the selected weight is bound to
    /// an identity.
    IdentityRefine,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_pinned() {
        for (wire, extra) in [
            ("face", RecipeExtra::Face),
            ("refine", RecipeExtra::Refine),
            ("identity_refine", RecipeExtra::IdentityRefine),
        ] {
            assert_eq!(
                serde_json::from_value::<RecipeExtra>(serde_json::json!(wire)).unwrap(),
                extra
            );
        }
    }
}
