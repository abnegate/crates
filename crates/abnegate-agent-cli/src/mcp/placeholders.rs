use std::collections::BTreeMap;
use std::io;

use abnegate_secret::SecretValue;
use tempfile::NamedTempFile;

use crate::mcp::attachment::McpAttachment;

/// The namespace every generated variable is named in. A remote server whose
/// URL or headers refer to any name in it never attaches, so what the file
/// moved out of one server's values never reaches another.
pub(crate) const NAMESPACE: &str = "ABNEGATE_MCP_";
const TOKEN_BYTES: usize = 16;
const OPENING: &str = "${";
const CLOSING: char = '}';
const DEFAULT: &str = ":-";

/// What a rendered MCP configuration leaves to the child's environment, each
/// value under a generated variable named `ABNEGATE_MCP_<token>_<n>`, whose
/// token is drawn at random for every rendering, so that no configuration can
/// name one in advance: literal text moved out of the file as it is, and a
/// stdio server's values that refer to variables, to be resolved before the
/// child is given them.
#[derive(Debug)]
pub(crate) struct Placeholders {
    prefix: String,
    pub(crate) environment: BTreeMap<String, SecretValue>,
    pub(crate) templates: BTreeMap<String, SecretValue>,
}

impl Placeholders {
    /// Placeholders named under a token drawn from the operating system's
    /// random source.
    pub(crate) fn new() -> io::Result<Self> {
        let mut token = [0; TOKEN_BYTES];
        getrandom::fill(&mut token).map_err(io::Error::from)?;
        let token: String = token.iter().map(|byte| format!("{byte:02X}")).collect();
        Ok(Self::under(&token))
    }

    /// Placeholders named `ABNEGATE_MCP_{token}_<n>`.
    pub(crate) fn under(token: &str) -> Self {
        Self {
            prefix: format!("{NAMESPACE}{token}_"),
            environment: BTreeMap::new(),
            templates: BTreeMap::new(),
        }
    }

    /// What to write in place of a stdio server's environment `value`:
    /// nothing for an empty value, and otherwise a reference to a generated
    /// variable holding it, as it is when it refers to no variable and to be
    /// resolved when it does.
    pub(crate) fn substitute(&mut self, value: &SecretValue) -> String {
        let text = value.expose();
        if text.is_empty() {
            return String::new();
        }
        if refers(text) {
            self.template(value.clone())
        } else {
            self.hold(text.to_string())
        }
    }

    /// What to write in place of a stdio server's command or argument: `text`
    /// itself when it refers to no variable, and otherwise a reference to a
    /// generated variable holding it, to be resolved.
    pub(crate) fn resolved(&mut self, text: &str) -> String {
        if refers(text) {
            self.template(SecretValue::new(text))
        } else {
            text.to_string()
        }
    }

    /// What to write in place of a remote server's `value`, whose references
    /// the CLI expands under rules of its own: each reference as written, for
    /// the CLI alone to expand, and each run of literal text around them as a
    /// reference to a generated variable holding it. Nothing a reference
    /// names is ever read from this process's environment.
    pub(crate) fn separate(&mut self, value: &SecretValue) -> String {
        let mut rendered = String::new();
        let mut literal = String::new();
        for segment in segments(value.expose()) {
            match segment {
                Segment::Literal(text) => literal.push_str(text),
                Segment::Reference { written, .. } => {
                    rendered.push_str(&self.hold(std::mem::take(&mut literal)));
                    rendered.push_str(written);
                }
            }
        }
        rendered.push_str(&self.hold(literal));
        rendered
    }

    /// The attachment for the rendered `file`, carrying every value moved
    /// out of it.
    pub(crate) fn attachment(self, file: NamedTempFile) -> McpAttachment {
        McpAttachment {
            file,
            environment: self.environment,
            templates: self.templates,
        }
    }

    /// A reference to a new generated variable holding `literal` as it is,
    /// or nothing when there is no text to hold.
    fn hold(&mut self, literal: String) -> String {
        if literal.is_empty() {
            return String::new();
        }
        let variable = self.generated();
        self.environment
            .insert(variable.clone(), SecretValue::new(literal));
        reference(&variable)
    }

    /// A reference to a new generated variable holding `template`, to be
    /// resolved before the child is given it.
    fn template(&mut self, template: SecretValue) -> String {
        let variable = self.generated();
        self.templates.insert(variable.clone(), template);
        reference(&variable)
    }

    /// The name of the next generated variable.
    fn generated(&self) -> String {
        format!(
            "{}{}",
            self.prefix,
            self.environment.len() + self.templates.len()
        )
    }
}

/// The variable each `${VAR}` or `${VAR:-default}` reference in `text`
/// names, in order.
pub(crate) fn references(text: &str) -> impl Iterator<Item = &str> {
    segments(text)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Reference { name, .. } => Some(name),
            Segment::Literal(_) => None,
        })
}

/// Whether `text` refers to any variable.
fn refers(text: &str) -> bool {
    references(text).next().is_some()
}

/// Whether `value` is nothing but one `${VAR}` reference, which names a
/// secret without holding one.
pub(crate) fn whole_reference(value: &str) -> bool {
    value
        .trim()
        .strip_prefix(OPENING)
        .and_then(|rest| rest.strip_suffix(CLOSING))
        .is_some_and(|name| !name.contains(OPENING) && !name.contains(CLOSING))
}

/// `template` with each `${VAR}` replaced by what `lookup` gives for it, and
/// each `${VAR:-default}` by its default when that is unset or empty, as
/// the CLI expands them. A reference to a variable nothing gives, with no
/// default, is left as written, as the CLI leaves it.
pub(crate) fn expand(template: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::with_capacity(template.len());
    for segment in segments(template) {
        match segment {
            Segment::Literal(text) => expanded.push_str(text),
            Segment::Reference {
                written,
                name,
                default,
            } => {
                let value = lookup(name).filter(|value| !value.is_empty() || default.is_none());
                match (value, default) {
                    (Some(value), _) => expanded.push_str(&value),
                    (None, Some(default)) => expanded.push_str(default),
                    (None, None) => {
                        tracing::warn!(
                            variable = name,
                            "an MCP value refers to a variable nothing sets; leaving it as written"
                        );
                        expanded.push_str(written);
                    }
                }
            }
        }
    }
    expanded
}

/// A run of literal text, or one `${VAR}` or `${VAR:-default}` reference as
/// `written`.
enum Segment<'text> {
    Literal(&'text str),
    Reference {
        written: &'text str,
        name: &'text str,
        default: Option<&'text str>,
    },
}

/// `text` as literal runs and references, in order. Anything shaped like a
/// reference that does not name a variable, or is never closed, is literal
/// text.
fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPENING) {
        let after = &rest[start + OPENING.len()..];
        let Some(end) = after.find(CLOSING) else {
            break;
        };
        let written = &rest[start..start + OPENING.len() + end + 1];
        let expression = &after[..end];
        let (name, default) = match expression.split_once(DEFAULT) {
            Some((name, default)) => (name, Some(default)),
            None => (expression, None),
        };
        if variable(name) {
            if start > 0 {
                segments.push(Segment::Literal(&rest[..start]));
            }
            segments.push(Segment::Reference {
                written,
                name,
                default,
            });
        } else {
            segments.push(Segment::Literal(&rest[..start + written.len()]));
        }
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest));
    }
    segments
}

/// `${variable}`.
fn reference(variable: &str) -> String {
    format!("{OPENING}{variable}{CLOSING}")
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

    use super::NAMESPACE;
    use super::Placeholders;
    use super::expand;
    use super::references;
    use super::whole_reference;

    #[test]
    fn a_literal_moves_to_a_generated_variable() {
        let mut placeholders = Placeholders::under("TEST");

        let first = placeholders.substitute(&SecretValue::new("glsa_realsecret"));
        let second = placeholders.substitute(&SecretValue::new("Bearer sk-live-secret"));

        assert_eq!(first, "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(second, "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_TEST_0")
                .map(SecretValue::expose),
            Some("glsa_realsecret")
        );
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_TEST_1")
                .map(SecretValue::expose),
            Some("Bearer sk-live-secret")
        );
        assert!(placeholders.templates.is_empty());
    }

    /// A reference left in the file would have the CLI read the variable
    /// from the child, which would then have to be handed it by name, where a
    /// remote server could name it too.
    #[test]
    fn a_whole_reference_moves_out_to_be_resolved() {
        let mut placeholders = Placeholders::under("TEST");

        for value in ["${APPWRITE_API_KEY}", "${CF_ID:-anonymous}"] {
            assert!(
                placeholders
                    .substitute(&SecretValue::new(value))
                    .starts_with("${ABNEGATE_MCP_TEST_"),
                "{value}"
            );
        }

        assert!(placeholders.environment.is_empty());
        assert_eq!(
            placeholders
                .templates
                .values()
                .map(SecretValue::expose)
                .collect::<Vec<_>>(),
            ["${APPWRITE_API_KEY}", "${CF_ID:-anonymous}"]
        );
    }

    #[test]
    fn a_value_mixing_references_with_literal_text_moves_out_as_a_template() {
        let mut placeholders = Placeholders::under("TEST");
        let blob = "{\"id\": \"${CF_ID}\", \"secret\": \"literal-cf-secret\"}";

        let first = placeholders.substitute(&SecretValue::new(blob));
        let second = placeholders.substitute(&SecretValue::new("Bearer ${TOKEN}"));

        assert_eq!(first, "${ABNEGATE_MCP_TEST_0}");
        assert_eq!(second, "${ABNEGATE_MCP_TEST_1}");
        assert_eq!(
            placeholders
                .templates
                .get("ABNEGATE_MCP_TEST_0")
                .map(SecretValue::expose),
            Some(blob)
        );
        assert!(placeholders.environment.is_empty());
    }

    #[test]
    fn a_command_or_argument_moves_out_only_when_it_refers_to_a_variable() {
        let mut placeholders = Placeholders::under("TEST");

        assert_eq!(placeholders.resolved("uvx"), "uvx");
        assert_eq!(placeholders.resolved("--verbose"), "--verbose");
        assert_eq!(
            placeholders.resolved("--token=${TOKEN}"),
            "${ABNEGATE_MCP_TEST_0}"
        );
        assert_eq!(
            placeholders
                .templates
                .get("ABNEGATE_MCP_TEST_0")
                .map(SecretValue::expose),
            Some("--token=${TOKEN}")
        );
        assert!(placeholders.environment.is_empty());
    }

    /// A configuration is written before it is rendered, so a name drawn
    /// afresh for every rendering is one no server can have been told.
    #[test]
    fn every_rendering_draws_names_no_configuration_can_know() {
        let mut first = Placeholders::new().expect("a random token");
        let mut second = Placeholders::new().expect("a random token");

        let first = first.substitute(&SecretValue::new("literal"));
        let second = second.substitute(&SecretValue::new("literal"));

        assert_ne!(first, second);
        for name in [&first, &second] {
            let token = name
                .strip_prefix(&format!("${{{NAMESPACE}"))
                .and_then(|name| name.strip_suffix("_0}"))
                .expect("a generated name");
            assert_eq!(token.len(), 32, "{name}");
            assert!(
                token
                    .chars()
                    .all(|character| character.is_ascii_hexdigit()
                        && !character.is_ascii_lowercase()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_template_expands_as_the_cli_would() {
        let lookup = |name: &str| match name {
            "TOKEN" => Some("tok".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };

        assert_eq!(expand("Bearer ${TOKEN}", &lookup), "Bearer tok");
        assert_eq!(
            expand("${MISSING:-anonymous}/${EMPTY:-fallback}", &lookup),
            "anonymous/fallback"
        );
        assert_eq!(expand("[${MISSING}][${EMPTY}]", &lookup), "[${MISSING}][]");
        assert_eq!(expand("${1BAD} and ${", &lookup), "${1BAD} and ${");
        assert_eq!(expand("no references", &lookup), "no references");
    }

    /// Claude Code starts a server with a reference to a variable nothing
    /// sets left as written, so every value expanded here on its behalf
    /// does the same, or one configuration would start a server with one
    /// value through the CLI and another through any other launcher.
    #[test]
    fn a_reference_to_a_variable_nothing_sets_is_left_as_written() {
        let unset = |_: &str| None;

        assert_eq!(expand("${TOKEN}", &unset), "${TOKEN}");
        assert_eq!(expand("Bearer ${TOKEN}", &unset), "Bearer ${TOKEN}");
        assert_eq!(expand("${TOKEN:-anonymous}", &unset), "anonymous");
    }

    #[test]
    fn only_a_single_whole_reference_counts_as_one() {
        assert!(whole_reference("${TOKEN}"));
        assert!(whole_reference("  ${TOKEN}  "));
        assert!(!whole_reference("Bearer ${TOKEN}"));
        assert!(!whole_reference("${A}${B}"));
        assert!(!whole_reference("${A}-${B}"));
        assert!(!whole_reference("literal"));
        assert!(!whole_reference("${"));
    }

    #[test]
    fn an_empty_value_or_a_broken_reference_is_never_mistaken_for_a_reference() {
        let mut placeholders = Placeholders::under("TEST");

        assert_eq!(placeholders.substitute(&SecretValue::new("")), "");
        assert_eq!(
            placeholders.substitute(&SecretValue::new("abc${")),
            "${ABNEGATE_MCP_TEST_0}"
        );
        assert_eq!(
            placeholders.substitute(&SecretValue::new("${1BAD} ${}")),
            "${ABNEGATE_MCP_TEST_1}"
        );
        assert_eq!(placeholders.environment.len(), 2);
        assert!(placeholders.templates.is_empty());
    }

    #[test]
    fn references_name_only_what_the_text_refers_to() {
        assert_eq!(references("mcp-server-appwrite").count(), 0);
        assert_eq!(
            references("--token=${TOKEN} ${ID:-anonymous} ${1BAD}").collect::<Vec<_>>(),
            ["TOKEN", "ID"]
        );
    }
}
