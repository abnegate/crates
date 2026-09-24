use serde::{Deserialize, Serialize};

use crate::wire::function_definition::FunctionDefinition;

const FUNCTION_TYPE: &str = "function";

/// A tool offered to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// The function `name`, described to the model as `description`, taking
    /// arguments that match the JSON schema `parameters`.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: FUNCTION_TYPE.to_string(),
            function: FunctionDefinition::new(name, description, parameters),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolDefinition;

    #[test]
    fn a_function_tool_names_itself_and_its_schema() {
        let definition = ToolDefinition::function(
            "read_file",
            "Read a file from disk",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        );
        assert_eq!(definition.tool_type, "function");
        assert_eq!(definition.function.name, "read_file");
        assert_eq!(definition.function.description, "Read a file from disk");
    }

    #[test]
    fn a_tool_definition_round_trips() {
        let definition = ToolDefinition::function(
            "search_files",
            "Search for files matching a pattern",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern"},
                    "directory": {"type": "string", "description": "Directory to search"}
                },
                "required": ["pattern"]
            }),
        );

        let json = serde_json::to_string(&definition).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tool_type, "function");
        assert_eq!(deserialized.function.name, "search_files");
        assert_eq!(
            deserialized.function.description,
            "Search for files matching a pattern"
        );
        assert!(deserialized.function.parameters.get("properties").is_some());
    }

    #[test]
    fn a_tool_without_parameters_round_trips() {
        let definition =
            ToolDefinition::function("get_time", "Get the current time", serde_json::json!({}));

        let json = serde_json::to_string(&definition).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.function.name, "get_time");
        assert!(
            deserialized
                .function
                .parameters
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_nested_parameter_schema_round_trips() {
        let definition = ToolDefinition::function(
            "complex_tool",
            "A tool with complex parameters",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "nested": {
                        "type": "object",
                        "properties": {
                            "inner": {"type": "string"}
                        }
                    },
                    "array_param": {
                        "type": "array",
                        "items": {"type": "integer"}
                    }
                }
            }),
        );

        let json = serde_json::to_string(&definition).unwrap();
        assert!(json.contains("nested"));
        assert!(json.contains("array_param"));

        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.function.name, "complex_tool");
    }
}
