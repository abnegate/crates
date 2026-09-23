use std::collections::BTreeMap;
use std::collections::BTreeSet;

use abnegate_secret::SecretValue;

const VARIABLE_PREFIX: &str = "ABNEGATE_MCP_";
const OPENING: &str = "${";
const CLOSING: char = '}';
const DEFAULT: &str = ":-";

/// What a rendered MCP configuration leaves to the child's environment: the
/// literal values moved out of the file, each under a generated variable,
/// and the host variables the file's own `${VAR}` references name.
#[derive(Debug, Default)]
pub(crate) struct Placeholders {
    pub(crate) environment: BTreeMap<String, SecretValue>,
    pub(crate) references: BTreeSet<String>,
}

impl Placeholders {
    /// What to write in place of `value`: a reference to a generated
    /// variable that holds it when it is a literal, or `value` itself when
    /// it already refers to variables of its own, or is empty.
    pub(crate) fn substitute(&mut self, value: &SecretValue) -> String {
        let text = value.expose();
        if text.is_empty() || self.note(text) {
            return text.to_string();
        }
        let variable = format!("{VARIABLE_PREFIX}{}", self.environment.len());
        let placeholder = format!("{OPENING}{variable}{CLOSING}");
        self.environment.insert(variable, value.clone());
        placeholder
    }

    /// Note every variable `text` refers to, and say whether it referred to
    /// any.
    pub(crate) fn note(&mut self, text: &str) -> bool {
        let mut found = false;
        let mut rest = text;
        while let Some(start) = rest.find(OPENING) {
            rest = &rest[start + OPENING.len()..];
            let Some(end) = rest.find(CLOSING) else {
                break;
            };
            let expression = &rest[..end];
            let name = expression
                .split_once(DEFAULT)
                .map_or(expression, |(name, _)| name);
            if variable(name) {
                self.references.insert(name.to_string());
                found = true;
            }
            rest = &rest[end + 1..];
        }
        found
    }
}

fn variable(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::Placeholders;

    #[test]
    fn a_literal_moves_to_a_generated_variable() {
        let mut placeholders = Placeholders::default();

        let first = placeholders.substitute(&SecretValue::new("glsa_realsecret"));
        let second = placeholders.substitute(&SecretValue::new("Bearer sk-live-secret"));

        assert_eq!(first, "${ABNEGATE_MCP_0}");
        assert_eq!(second, "${ABNEGATE_MCP_1}");
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_0")
                .map(SecretValue::expose),
            Some("glsa_realsecret")
        );
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_1")
                .map(SecretValue::expose),
            Some("Bearer sk-live-secret")
        );
        assert!(placeholders.references.is_empty());
    }

    #[test]
    fn a_value_with_references_stays_and_its_variables_are_noted() {
        let mut placeholders = Placeholders::default();

        for value in [
            "${APPWRITE_API_KEY}",
            "Bearer ${TOKEN}",
            "{\"id\": \"${CF_ID:-anonymous}\", \"secret\": \"${CF_SECRET}\"}",
        ] {
            assert_eq!(placeholders.substitute(&SecretValue::new(value)), value);
        }

        assert!(placeholders.environment.is_empty());
        assert_eq!(
            placeholders.references.iter().collect::<Vec<_>>(),
            ["APPWRITE_API_KEY", "CF_ID", "CF_SECRET", "TOKEN"]
        );
    }

    #[test]
    fn an_empty_value_or_a_broken_reference_is_never_mistaken_for_a_reference() {
        let mut placeholders = Placeholders::default();

        assert_eq!(placeholders.substitute(&SecretValue::new("")), "");
        assert_eq!(
            placeholders.substitute(&SecretValue::new("abc${")),
            "${ABNEGATE_MCP_0}"
        );
        assert_eq!(
            placeholders.substitute(&SecretValue::new("${1BAD} ${}")),
            "${ABNEGATE_MCP_1}"
        );
        assert!(placeholders.references.is_empty());
    }

    #[test]
    fn noting_plain_text_notes_nothing() {
        let mut placeholders = Placeholders::default();
        assert!(!placeholders.note("mcp-server-appwrite"));
        assert!(placeholders.note("--token=${TOKEN}"));
        assert!(placeholders.references.contains("TOKEN"));
    }
}
