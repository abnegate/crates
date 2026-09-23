use regex::{Captures, Regex};
use serde_json::Value;
use std::cell::RefCell;
use std::sync::LazyLock;

use super::{TemplateContext, TemplateError};

/// `{{#if key}}…{{/if}}`: kept when the key holds a truthy value.
static CONDITIONAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{#if\s+([\w-]+)\}\}([\s\S]*?)\{\{/if\}\}").expect("a valid conditional pattern")
});

/// A `{{#each key}}…{{/each}}` loop, or a `{{key}}` placeholder.
static EXPANSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{#each\s+([\w-]+)\}\}([\s\S]*?)\{\{/each\}\}|\{\{\s*([@\w.-]+)\s*\}\}")
        .expect("a valid expansion pattern")
});

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([@\w.-]+)\s*\}\}").expect("a valid placeholder pattern"));

const THIS: &str = "this";
const THIS_FIELD: &str = "this.";
const ITEM: &str = "item";
const INDEX: &str = "@index";
const FIRST: &str = "@first";
const LAST: &str = "@last";

/// Renders templates against a [`TemplateContext`].
///
/// `{{key}}` becomes the key's value, and a key the context does not hold
/// renders as nothing; [`render_strict`](Self::render_strict) refuses such a
/// template instead. Keys are letters, digits, `_` and `-`.
/// `{{#if key}}…{{/if}}` keeps its body when the key is truthy: `true`, a
/// non-empty string, array or object, or any number; an empty string, like a
/// missing key, is false. `{{#each key}}…{{/each}}` repeats its body for every
/// item of an array: an object item's fields are `{{field}}` and
/// `{{this.field}}`, a scalar item is `{{this}}` and `{{item}}`, an object
/// item's `item` field is also `{{this}}`, and `{{@index}}`, `{{@first}}` and
/// `{{@last}}` say where the loop is. A key the item does not hold falls back
/// to the context.
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateRenderer;

impl TemplateRenderer {
    pub fn new() -> Self {
        Self
    }

    /// Render `template`, with every key the context lacks as nothing.
    pub fn render(&self, template: &str, context: &TemplateContext) -> String {
        expand(template, context, &RefCell::new(Vec::new()))
    }

    /// Render `template`, or name the first key it uses that the context
    /// does not hold.
    ///
    /// A key tested by `{{#if}}` is never missing: absence is what the test
    /// is for.
    pub fn render_strict(
        &self,
        template: &str,
        context: &TemplateContext,
    ) -> Result<String, TemplateError> {
        let missing = RefCell::new(Vec::new());
        let rendered = expand(template, context, &missing);
        match missing.into_inner().into_iter().next() {
            Some(key) => Err(TemplateError::Missing(key)),
            None => Ok(rendered),
        }
    }
}

fn expand(template: &str, context: &TemplateContext, missing: &RefCell<Vec<String>>) -> String {
    let kept = CONDITIONAL.replace_all(template, |captures: &Captures<'_>| {
        if context.get(&captures[1]).is_some_and(truthy) {
            captures[2].to_string()
        } else {
            String::new()
        }
    });
    EXPANSION
        .replace_all(&kept, |captures: &Captures<'_>| match captures.get(1) {
            Some(array) => iterate(array.as_str(), &captures[2], context, missing),
            None => lookup(&captures[3], context.get(&captures[3]).map(text), missing),
        })
        .into_owned()
}

/// A key's text, or nothing, noting the key when there was none.
fn lookup(key: &str, value: Option<String>, missing: &RefCell<Vec<String>>) -> String {
    value.unwrap_or_else(|| {
        missing.borrow_mut().push(key.to_string());
        String::new()
    })
}

fn iterate(
    array: &str,
    body: &str,
    context: &TemplateContext,
    missing: &RefCell<Vec<String>>,
) -> String {
    let Some(value) = context.get(array) else {
        missing.borrow_mut().push(array.to_string());
        return String::new();
    };
    let Some(items) = value.as_array() else {
        return String::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            PLACEHOLDER
                .replace_all(body, |captures: &Captures<'_>| {
                    let key = &captures[1];
                    let value = scoped(key, item, index, items.len())
                        .or_else(|| context.get(key).map(text));
                    lookup(key, value, missing)
                })
                .into_owned()
        })
        .collect()
}

/// What `key` means inside one iteration of a loop, if the loop defines it.
fn scoped(key: &str, item: &Value, index: usize, count: usize) -> Option<String> {
    match key {
        INDEX => Some(index.to_string()),
        FIRST => Some((index == 0).to_string()),
        LAST => Some((index + 1 == count).to_string()),
        THIS => Some(match item {
            Value::Object(fields) => fields.get(ITEM).map_or_else(|| item.to_string(), text),
            scalar => text(scalar),
        }),
        _ => {
            let field = key.strip_prefix(THIS_FIELD).unwrap_or(key);
            match item {
                Value::Object(fields) => fields.get(field).map(text),
                scalar if field == ITEM => Some(text(scalar)),
                _ => None,
            }
        }
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(_) => true,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// The context an issue tracker would render a fix prompt from.
    fn issue(context: &str) -> TemplateContext {
        TemplateContext::new()
            .with_variable("id", "123")
            .with_variable("short_id", "PROJ-123")
            .with_variable("title", "Fix the bug")
            .with_variable("url", "https://example.com/issue/123")
            .with_variable("source", "linear")
            .with_variable("description", "This is a bug description")
            .with_variable("context", context)
            .with_value("has_agent_md", false)
            .with_value("agent_md", Value::Null)
    }

    fn with_agent_md(context: TemplateContext, content: &str) -> TemplateContext {
        context
            .with_value("has_agent_md", true)
            .with_variable("agent_md", content)
    }

    #[test]
    fn test_basic_variable_substitution() {
        let renderer = TemplateRenderer::new();
        let context = issue("Stack trace here");

        let template = "Fix {{short_id}}: {{title}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Fix PROJ-123: Fix the bug");
    }

    #[test]
    fn test_context_substitution() {
        let renderer = TemplateRenderer::new();
        let context = issue("Error at line 42");

        let template = "Context:\n{{context}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Context:\nError at line 42");
    }

    #[test]
    fn test_agent_md_substitution() {
        let renderer = TemplateRenderer::new();
        let context = with_agent_md(issue(""), "Custom instructions");

        let template = "{{#if has_agent_md}}{{agent_md}}\n---\n{{/if}}Fix the issue";
        let result = renderer.render(template, &context);

        assert!(result.contains("Custom instructions"));
        assert!(result.contains("---"));
    }

    #[test]
    fn test_conditional_without_agent_md() {
        let renderer = TemplateRenderer::new();
        let context = issue("");

        let template = "{{#if has_agent_md}}AGENT.md present\n{{/if}}Main content";
        let result = renderer.render(template, &context);

        assert!(!result.contains("AGENT.md present"));
        assert!(result.contains("Main content"));
    }

    #[test]
    fn test_conditional_with_description() {
        let renderer = TemplateRenderer::new();
        let context = issue("");

        let template = "{{#if description}}Description: {{description}}{{/if}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("Description: This is a bug description"));
    }

    #[test]
    fn test_custom_variables() {
        let renderer = TemplateRenderer::new();
        let context = issue("").with_variable("branch", "fix/bug-123");

        let template = "Branch: {{branch}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Branch: fix/bug-123");
    }

    #[test]
    fn test_full_template() {
        let renderer = TemplateRenderer::new();
        let context = with_agent_md(
            issue("Error: NullPointerException"),
            "Follow coding standards",
        );

        let template = r#"{{#if has_agent_md}}
{{agent_md}}

---
{{/if}}
Fix issue {{short_id}}: {{title}}
Source: {{source}}
URL: {{url}}

{{#if description}}
Description:
{{description}}
{{/if}}

Context:
{{context}}"#;

        let result = renderer.render(template, &context);

        assert!(result.contains("Follow coding standards"));
        assert!(result.contains("Fix issue PROJ-123: Fix the bug"));
        assert!(result.contains("Source: linear"));
        assert!(result.contains("Description:"));
        assert!(result.contains("This is a bug description"));
        assert!(result.contains("Error: NullPointerException"));
    }

    #[test]
    fn test_each_basic_array() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([
                ("name".to_string(), "Alice".to_string()),
                ("role".to_string(), "Developer".to_string()),
            ]),
            HashMap::from([
                ("name".to_string(), "Bob".to_string()),
                ("role".to_string(), "Designer".to_string()),
            ]),
        ];

        let context = issue("").with_array("users", items);

        let template = "Users:{{#each users}}\n- {{name}} ({{role}}){{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("- Alice (Developer)"));
        assert!(result.contains("- Bob (Designer)"));
    }

    #[test]
    fn test_each_with_this_property() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([("name".to_string(), "Item1".to_string())]),
            HashMap::from([("name".to_string(), "Item2".to_string())]),
        ];

        let context = issue("").with_array("items", items);

        let template = "{{#each items}}{{this.name}} {{/each}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Item1 Item2 ");
    }

    #[test]
    fn test_each_with_index() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([("name".to_string(), "First".to_string())]),
            HashMap::from([("name".to_string(), "Second".to_string())]),
            HashMap::from([("name".to_string(), "Third".to_string())]),
        ];

        let context = issue("").with_array("items", items);

        let template = "{{#each items}}{{@index}}: {{name}}\n{{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("0: First"));
        assert!(result.contains("1: Second"));
        assert!(result.contains("2: Third"));
    }

    #[test]
    fn test_each_with_first_last() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([("name".to_string(), "A".to_string())]),
            HashMap::from([("name".to_string(), "B".to_string())]),
            HashMap::from([("name".to_string(), "C".to_string())]),
        ];

        let context = issue("").with_array("items", items);

        let template = "{{#each items}}{{name}}(first={{@first}},last={{@last}}) {{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("A(first=true,last=false)"));
        assert!(result.contains("B(first=false,last=false)"));
        assert!(result.contains("C(first=false,last=true)"));
    }

    #[test]
    fn test_each_empty_array() {
        let renderer = TemplateRenderer::new();
        let context = issue("").with_array("items", vec![]);

        let template = "Before{{#each items}}\n- {{name}}{{/each}}After";
        let result = renderer.render(template, &context);

        assert_eq!(result, "BeforeAfter");
    }

    #[test]
    fn test_each_nonexistent_array() {
        let renderer = TemplateRenderer::new();
        let context = issue("");

        let template = "Before{{#each missing}}ITEM{{/each}}After";
        let result = renderer.render(template, &context);

        assert_eq!(result, "BeforeAfter");
    }

    #[test]
    fn test_each_simple_string_array() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([("item".to_string(), "apple".to_string())]),
            HashMap::from([("item".to_string(), "banana".to_string())]),
            HashMap::from([("item".to_string(), "cherry".to_string())]),
        ];

        let context = issue("").with_array("fruits", items);

        let template = "Fruits: {{#each fruits}}{{this}}, {{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("apple"));
        assert!(result.contains("banana"));
        assert!(result.contains("cherry"));
    }

    #[test]
    fn test_each_with_json_array() {
        let renderer = TemplateRenderer::new();
        let json: serde_json::Value = serde_json::json!([
            {"file": "src/main.rs", "line": 42},
            {"file": "src/lib.rs", "line": 100}
        ]);

        let context = issue("").with_json_array("stack_frames", &json);

        let template = "Stack:{{#each stack_frames}}\n  {{file}}:{{line}}{{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("src/main.rs:42"));
        assert!(result.contains("src/lib.rs:100"));
    }

    #[test]
    fn test_each_with_json_string_array() {
        let renderer = TemplateRenderer::new();
        let json: serde_json::Value = serde_json::json!(["tag1", "tag2", "tag3"]);

        let context = issue("").with_json_array("tags", &json);

        let template = "Tags: {{#each tags}}#{{this}} {{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("#tag1"));
        assert!(result.contains("#tag2"));
        assert!(result.contains("#tag3"));
    }

    #[test]
    fn test_each_multiple_loops() {
        let renderer = TemplateRenderer::new();
        let users = vec![
            HashMap::from([("name".to_string(), "Alice".to_string())]),
            HashMap::from([("name".to_string(), "Bob".to_string())]),
        ];
        let tasks = vec![
            HashMap::from([("task".to_string(), "Fix bug".to_string())]),
            HashMap::from([("task".to_string(), "Add feature".to_string())]),
        ];

        let context = issue("")
            .with_array("users", users)
            .with_array("tasks", tasks);

        let template =
            "Users:{{#each users}} {{name}}{{/each}}\nTasks:{{#each tasks}} {{task}}{{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("Users: Alice Bob"));
        assert!(result.contains("Tasks: Fix bug Add feature"));
    }

    #[test]
    fn test_each_with_special_characters() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([("text".to_string(), "Hello <World>".to_string())]),
            HashMap::from([("text".to_string(), "Test & Debug".to_string())]),
        ];

        let context = issue("").with_array("items", items);

        let template = "{{#each items}}{{text}}\n{{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("Hello <World>"));
        assert!(result.contains("Test & Debug"));
    }

    #[test]
    fn test_if_array_exists() {
        let renderer = TemplateRenderer::new();
        let items = vec![HashMap::from([("name".to_string(), "Item".to_string())])];

        let context = issue("").with_array("items", items);

        let template = "{{#if items}}Has items{{/if}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Has items");
    }

    #[test]
    fn test_if_empty_array_not_shown() {
        let renderer = TemplateRenderer::new();
        let context = issue("").with_array("items", vec![]);

        let template = "{{#if items}}Has items{{/if}}Empty";
        let result = renderer.render(template, &context);

        assert_eq!(result, "Empty");
    }

    #[test]
    fn test_each_with_multiline_content() {
        let renderer = TemplateRenderer::new();
        let items = vec![
            HashMap::from([
                ("title".to_string(), "Issue 1".to_string()),
                ("desc".to_string(), "Description 1".to_string()),
            ]),
            HashMap::from([
                ("title".to_string(), "Issue 2".to_string()),
                ("desc".to_string(), "Description 2".to_string()),
            ]),
        ];

        let context = issue("").with_array("issues", items);

        let template = r#"Issues:
{{#each issues}}
## {{title}}
{{desc}}

{{/each}}"#;
        let result = renderer.render(template, &context);

        assert!(result.contains("## Issue 1\nDescription 1"));
        assert!(result.contains("## Issue 2\nDescription 2"));
    }

    #[test]
    fn test_each_single_item() {
        let renderer = TemplateRenderer::new();
        let items = vec![HashMap::from([("name".to_string(), "Only".to_string())])];

        let context = issue("").with_array("items", items);

        let template = "{{#each items}}{{@first}}-{{@last}}-{{name}}{{/each}}";
        let result = renderer.render(template, &context);

        assert_eq!(result, "true-true-Only");
    }

    #[test]
    fn test_each_combined_with_if() {
        let renderer = TemplateRenderer::new();
        let items = vec![HashMap::from([("name".to_string(), "Test".to_string())])];

        let context = with_agent_md(issue("").with_array("items", items), "Guidelines");

        let template = r#"{{#if has_agent_md}}{{agent_md}}{{/if}}
Items:{{#each items}} {{name}}{{/each}}"#;
        let result = renderer.render(template, &context);

        assert!(result.contains("Guidelines"));
        assert!(result.contains("Items: Test"));
    }

    #[test]
    fn test_each_with_numeric_values() {
        let renderer = TemplateRenderer::new();
        let json: serde_json::Value = serde_json::json!([
            {"count": 10, "name": "errors"},
            {"count": 5, "name": "warnings"}
        ]);

        let context = issue("").with_json_array("metrics", &json);

        let template = "{{#each metrics}}{{name}}: {{count}}\n{{/each}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("errors: 10"));
        assert!(result.contains("warnings: 5"));
    }

    #[test]
    fn test_each_preserves_surrounding_template() {
        let renderer = TemplateRenderer::new();
        let items = vec![HashMap::from([("x".to_string(), "A".to_string())])];

        let context = issue("ctx").with_array("items", items);

        let template = "Issue: {{short_id}}\n{{#each items}}{{x}}{{/each}}\nContext: {{context}}";
        let result = renderer.render(template, &context);

        assert!(result.contains("Issue: PROJ-123"));
        assert!(result.contains("A"));
        assert!(result.contains("Context: ctx"));
    }

    /// A value is inserted, never re-read: an issue title that quotes a
    /// placeholder must not pull another value into the prompt.
    #[test]
    fn a_value_is_never_read_as_template_text() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new()
            .with_variable("title", "Leaks {{token}} and {{#if token}}more{{/if}}")
            .with_variable("token", "hunter2")
            .with_array(
                "items",
                vec![HashMap::from([(
                    "name".to_string(),
                    "{{token}}".to_string(),
                )])],
            );

        let result = renderer.render("{{title}} {{#each items}}{{name}}{{/each}}", &context);

        assert_eq!(
            result,
            "Leaks {{token}} and {{#if token}}more{{/if}} {{token}}"
        );
    }

    #[test]
    fn a_placeholder_the_context_does_not_hold_renders_as_nothing() {
        let renderer = TemplateRenderer::new();
        let result = renderer.render(
            "Hello {{name}}, {{missing}}",
            &issue("").with_variable("name", "Ada"),
        );
        assert_eq!(result, "Hello Ada, ");
    }

    /// Strictness is for a caller who would rather hear about a key it
    /// forgot to set than send a prompt with a hole in it.
    #[test]
    fn a_strict_render_names_the_first_key_the_context_lacks() {
        let renderer = TemplateRenderer::new();
        let context = issue("").with_variable("name", "Ada");

        assert_eq!(
            renderer.render_strict("Hello {{name}}, {{missing}} {{other}}", &context),
            Err(TemplateError::Missing("missing".to_string()))
        );
        assert_eq!(
            renderer.render_strict("{{#each absent}}x{{/each}}", &context),
            Err(TemplateError::Missing("absent".to_string()))
        );
        assert_eq!(
            renderer.render_strict("Hello {{name}}{{#if missing}}!{{/if}}", &context),
            Ok("Hello Ada".to_string()),
            "an {{#if}} on a missing key is a test, not a hole"
        );
    }

    /// Keys like `issue-id` were left as written, braces and all.
    #[test]
    fn a_key_may_contain_a_hyphen() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new()
            .with_variable("issue-id", "PROJ-7")
            .with_value("has-notes", true)
            .with_value("tag-list", json!(["a", "b"]));

        let result = renderer.render(
            "{{issue-id}}{{#if has-notes}} +notes{{/if}}{{#each tag-list}} #{{this}}{{/each}}",
            &context,
        );

        assert_eq!(result, "PROJ-7 +notes #a #b");
    }

    #[test]
    fn an_empty_string_is_false() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new().with_variable("blank", "");
        assert_eq!(renderer.render("{{#if blank}}shown{{/if}}", &context), "");
    }

    #[test]
    fn scalar_values_render_as_their_text_and_null_as_nothing() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new()
            .with_value("count", 3)
            .with_value("ratio", 0.5)
            .with_value("ready", true)
            .with_value("owner", Value::Null);

        let result = renderer.render("{{count}}|{{ratio}}|{{ready}}|{{owner}}|", &context);

        assert_eq!(result, "3|0.5|true||");
    }

    #[test]
    fn truthiness_follows_the_kind_of_value() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new()
            .with_value("zero", 0)
            .with_value("off", false)
            .with_value("blank", "")
            .with_value("none", Value::Null)
            .with_value("empty", json!({}))
            .with_value("full", json!({"a": 1}));
        let template = "{{#if zero}}zero{{/if}}{{#if off}}off{{/if}}{{#if blank}}blank{{/if}}\
                        {{#if none}}none{{/if}}{{#if empty}}empty{{/if}}{{#if full}}full{{/if}}\
                        {{#if missing}}missing{{/if}}";

        assert_eq!(renderer.render(template, &context), "zerofull");
    }

    #[test]
    fn a_loop_over_scalars_names_each_as_this_and_item() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new().with_value("ports", json!([80, 443]));

        let result = renderer.render("{{#each ports}}{{this}}/{{item}} {{/each}}", &context);

        assert_eq!(result, "80/80 443/443 ");
    }

    #[test]
    fn a_loop_field_the_item_lacks_falls_back_to_the_context() {
        let renderer = TemplateRenderer::new();
        let context = TemplateContext::new()
            .with_variable("repository", "crates")
            .with_value("files", json!([{"path": "a.rs"}, {"path": "b.rs"}]));

        let result = renderer.render("{{#each files}}{{repository}}/{{path}} {{/each}}", &context);

        assert_eq!(result, "crates/a.rs crates/b.rs ");
    }
}
