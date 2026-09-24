use serde::{Deserialize, Serialize};

use crate::wire::specific_function::SpecificFunction;
use crate::wire::tool_mode::ToolMode;

const FUNCTION_TYPE: &str = "function";

/// Whether, and which, tool the model must call.
///
/// On the wire a mode is a bare string (`"auto"`, `"none"`, `"required"`)
/// and a forced tool is an object naming it. Each string reads back as the
/// mode it names, and a string that names no mode is refused rather than
/// read as some other one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ToolChoice {
    Mode(ToolMode),
    /// Force a specific tool.
    Specific {
        r#type: String,
        function: SpecificFunction,
    },
}

impl ToolChoice {
    pub fn auto() -> Self {
        Self::Mode(ToolMode::Auto)
    }

    pub fn none() -> Self {
        Self::Mode(ToolMode::None)
    }

    pub fn required() -> Self {
        Self::Mode(ToolMode::Required)
    }

    pub fn specific(name: impl Into<String>) -> Self {
        Self::Specific {
            r#type: FUNCTION_TYPE.to_string(),
            function: SpecificFunction::new(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolChoice;

    #[test]
    fn a_mode_serialises_as_a_bare_string() {
        assert_eq!(
            serde_json::to_string(&ToolChoice::auto()).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&ToolChoice::none()).unwrap(),
            "\"none\""
        );
        assert_eq!(
            serde_json::to_string(&ToolChoice::required()).unwrap(),
            "\"required\""
        );
    }

    #[test]
    fn a_forced_tool_serialises_as_an_object_naming_it() {
        let json = serde_json::to_value(ToolChoice::specific("my_function")).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "my_function");
    }

    #[test]
    fn every_choice_reads_back_as_itself() {
        for choice in [
            ToolChoice::auto(),
            ToolChoice::none(),
            ToolChoice::required(),
            ToolChoice::specific("read_file"),
        ] {
            let json = serde_json::to_string(&choice).unwrap();
            assert_eq!(
                serde_json::from_str::<ToolChoice>(&json).unwrap(),
                choice,
                "{json}"
            );
        }
    }

    #[test]
    fn none_is_never_read_as_auto() {
        assert_eq!(
            serde_json::from_str::<ToolChoice>("\"none\"").unwrap(),
            ToolChoice::none()
        );
    }

    #[test]
    fn a_string_that_names_no_mode_is_refused() {
        assert!(serde_json::from_str::<ToolChoice>("\"sometimes\"").is_err());
    }
}
