use serde::{Deserialize, Serialize};

/// Who a message came from.
///
/// A role may be added in a minor release, so a `match` outside this crate
/// needs an arm for the ones it does not know:
///
/// ```compile_fail,E0004
/// use abnegate_llm::Role;
///
/// fn label(role: Role) -> &'static str {
///     match role {
///         Role::System => "system",
///         Role::User => "user",
///         Role::Assistant => "assistant",
///         Role::Tool => "tool",
///     }
/// }
/// # let _ = label(Role::User);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[cfg(test)]
mod tests {
    use super::Role;

    #[test]
    fn roles_serialise_as_their_wire_names() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
    }

    #[test]
    fn roles_deserialise_from_their_wire_names() {
        let system: Role = serde_json::from_str("\"system\"").unwrap();
        let user: Role = serde_json::from_str("\"user\"").unwrap();
        let assistant: Role = serde_json::from_str("\"assistant\"").unwrap();
        let tool: Role = serde_json::from_str("\"tool\"").unwrap();

        assert_eq!(system, Role::System);
        assert_eq!(user, Role::User);
        assert_eq!(assistant, Role::Assistant);
        assert_eq!(tool, Role::Tool);
    }

    #[test]
    fn roles_compare_by_value() {
        assert_eq!(Role::System, Role::System);
        assert_eq!(Role::User, Role::User);
        assert_ne!(Role::System, Role::User);
        assert_ne!(Role::Assistant, Role::Tool);
    }

    #[test]
    fn a_role_is_copied_rather_than_moved() {
        let role = Role::Assistant;
        let copied = role;
        assert_eq!(role, copied);
    }
}
