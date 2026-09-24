use std::collections::HashMap;

use serde_json::Map;
use serde_json::Value;

/// The values a template is rendered against, by key.
///
/// A key holds any JSON value. A string, number or boolean renders as its
/// text and `null` renders as nothing; an array is what `{{#each}}` iterates.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TemplateContext {
    /// The value each key renders as.
    pub values: HashMap<String, Value>,
}

impl TemplateContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `key` to any JSON value.
    pub fn with_value(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }

    /// Set `key` to a string.
    pub fn with_variable(self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.with_value(key, Value::String(value.into()))
    }

    /// Set `key` to a list of string maps for `{{#each}}` to iterate.
    pub fn with_array(self, key: impl Into<String>, items: Vec<HashMap<String, String>>) -> Self {
        let items: Vec<Value> = items
            .into_iter()
            .map(|item| {
                Value::Object(
                    item.into_iter()
                        .map(|(field, value)| (field, Value::String(value)))
                        .collect::<Map<String, Value>>(),
                )
            })
            .collect();
        self.with_value(key, items)
    }

    /// Set `key` to a JSON array for `{{#each}}` to iterate. Anything but an
    /// array is ignored.
    pub fn with_json_array(self, key: impl Into<String>, value: &Value) -> Self {
        if value.is_array() {
            self.with_value(key, value.clone())
        } else {
            self
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }
}

impl From<HashMap<String, Value>> for TemplateContext {
    fn from(values: HashMap<String, Value>) -> Self {
        Self { values }
    }
}

impl From<Map<String, Value>> for TemplateContext {
    fn from(values: Map<String, Value>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_json_object_becomes_a_context_of_its_fields() {
        let Value::Object(fields) = json!({"title": "Fix", "count": 3}) else {
            unreachable!("the literal is an object");
        };
        let context = TemplateContext::from(fields);
        assert_eq!(context.get("title"), Some(&json!("Fix")));
        assert_eq!(context.get("count"), Some(&json!(3)));
    }

    #[test]
    fn a_non_array_json_value_is_not_an_array() {
        let context = TemplateContext::new().with_json_array("tags", &json!("solo"));
        assert!(context.get("tags").is_none());
    }

    #[test]
    fn an_array_of_string_maps_becomes_an_array_of_objects() {
        let context = TemplateContext::new().with_array(
            "users",
            vec![HashMap::from([("name".to_string(), "Alice".to_string())])],
        );
        assert_eq!(context.get("users"), Some(&json!([{"name": "Alice"}])));
    }
}
