//! Translating each agent's own stream format into [`AgentEvent`](crate::AgentEvent).

pub mod claude;
pub mod codex;

const QUOTE: u8 = b'"';
const ESCAPE: u8 = b'\\';
const SEPARATOR: u8 = b':';
const TYPE: &str = "type";

/// Every `"type"` in `prefix`, the start of a JSON object too long to parse,
/// with the depth of the object or array it sits in, in order.
///
/// A top-level `"type"` sits at depth 1. A `"type"` inside a string, such as
/// one quoted in a tool's output, is escaped there and never counted.
pub(crate) fn types(prefix: &str) -> Vec<(usize, &str)> {
    let bytes = prefix.as_bytes();
    let mut found = Vec::new();
    let mut depth: usize = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            QUOTE => {
                let Some((key, after)) = string(prefix, index) else {
                    break;
                };
                index = after;
                if key == TYPE
                    && let Some(colon) = next(bytes, index).filter(|&at| bytes[at] == SEPARATOR)
                    && let Some(opening) = next(bytes, colon + 1).filter(|&at| bytes[at] == QUOTE)
                    && let Some((value, after)) = string(prefix, opening)
                {
                    found.push((depth, value));
                    index = after;
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    found
}

/// The contents of the string opening at `opening`, and the index just past
/// its closing quote, or `None` when the prefix ends inside it.
fn string(text: &str, opening: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let mut index = opening + 1;
    while index < bytes.len() {
        match bytes[index] {
            ESCAPE => index += 2,
            QUOTE => return Some((&text[opening + 1..index], index + 1)),
            _ => index += 1,
        }
    }
    None
}

/// The index of the next byte from `index` that is not whitespace.
fn next(bytes: &[u8], index: usize) -> Option<usize> {
    (index..bytes.len()).find(|&at| !bytes[at].is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use super::types;

    #[test]
    fn a_top_level_type_and_the_types_nested_in_it_are_found_by_depth() {
        assert_eq!(
            types(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"#
            ),
            [(1, "user"), (4, "tool_result")]
        );
        assert_eq!(
            types(
                r#"{ "type" : "item.completed", "item": {"id":"item_2","type":"agent_message","text":"#
            ),
            [(1, "item.completed"), (2, "agent_message")]
        );
    }

    #[test]
    fn a_type_quoted_inside_a_string_is_never_counted() {
        assert_eq!(
            types(r#"{"type":"user","content":"{\"type\":\"result\"} and \"type\": \"assistant\""#),
            [(1, "user")]
        );
    }

    #[test]
    fn a_prefix_cut_inside_a_string_or_that_is_not_json_yields_what_came_before() {
        assert_eq!(
            types(r#"{"type":"assistant","message":{"content":[{"text":"cut he"#),
            [(1, "assistant")]
        );
        assert_eq!(types(r#"{"typ"#), []);
        assert_eq!(types("xxxxxxxx"), []);
        assert_eq!(types(r#"{"type":42}"#), []);
    }
}
