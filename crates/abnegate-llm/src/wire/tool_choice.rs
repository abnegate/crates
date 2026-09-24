use serde::{Deserialize, Serialize};

use crate::wire::FUNCTION_TYPE;
use crate::wire::specific_function::SpecificFunction;
use crate::wire::tool_mode::ToolMode;

/// Whether, and which, tool the model must call.
///
/// On the wire a mode is a bare string (`"auto"`, `"none"`, `"required"`)
/// and a forced tool is an object naming it. Each string reads back as the
/// mode it names, and a string that names no mode is refused rather than
/// read as some other one.
///
/// [`ToolChoice::Specific`] may gain a field in a minor release, so it is
/// built with [`ToolChoice::specific`] and a pattern outside this crate ends
/// in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_llm::ToolChoice;
///
/// fn forced(choice: &ToolChoice) -> Option<&str> {
///     match choice {
///         ToolChoice::Specific { r#type: _, function } => Some(&function.name),
///         _ => None,
///     }
/// }
/// # let _ = forced(&ToolChoice::specific("read_file"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ToolChoice {
    /// Leave the choice to the model, forbid tools, or require one.
    Mode(ToolMode),
    /// Force a specific tool.
    #[non_exhaustive]
    Specific {
        r#type: String,
        function: SpecificFunction,
    },
}

impl ToolChoice {
    /// The model decides whether to call a tool.
    pub fn auto() -> Self {
        Self::Mode(ToolMode::Auto)
    }

    /// The model calls no tool.
    pub fn none() -> Self {
        Self::Mode(ToolMode::None)
    }

    /// The model calls at least one tool.
    pub fn required() -> Self {
        Self::Mode(ToolMode::Required)
    }

    /// The model calls the function `name`.
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
