use serde::{Deserialize, Serialize};

use crate::wire::specific_function::SpecificFunction;

/// Whether, and which, tool the model must call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Let the model decide.
    Auto(String),
    /// Do not use tools.
    None(String),
    /// Force a specific tool.
    Specific {
        r#type: String,
        function: SpecificFunction,
    },
}

impl ToolChoice {
    pub fn auto() -> Self {
        Self::Auto("auto".to_string())
    }

    pub fn none() -> Self {
        Self::None("none".to_string())
    }

    pub fn specific(name: impl Into<String>) -> Self {
        Self::Specific {
            r#type: "function".to_string(),
            function: SpecificFunction { name: name.into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolChoice;

    #[test]
    fn auto_serialises_as_a_bare_string() {
        assert_eq!(
            serde_json::to_string(&ToolChoice::auto()).unwrap(),
            "\"auto\""
        );
    }

    #[test]
    fn none_serialises_as_a_bare_string() {
        assert_eq!(
            serde_json::to_string(&ToolChoice::none()).unwrap(),
            "\"none\""
        );
    }

    #[test]
    fn a_forced_tool_serialises_as_an_object_naming_it() {
        let json = serde_json::to_string(&ToolChoice::specific("my_function")).unwrap();
        assert!(json.contains("function"));
        assert!(json.contains("my_function"));
    }
}
