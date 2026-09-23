use std::collections::HashSet;

/// `server` + `tool` → a function name safe for OpenAI-style tool calling.
pub fn qualified_tool_name(server: &str, tool: &str) -> String {
    let server = sanitize_identifier(server);
    let tool = sanitize_identifier(tool);
    if tool == server || tool.starts_with(&format!("{server}_")) {
        tool
    } else {
        format!("{server}_{tool}")
    }
}

/// Same as [`qualified_tool_name`], then `_2`, `_3`, … if that name is taken.
///
/// Sanitizing `list.files` and `list_files`, or prefix-skipping `srv_ping`
/// next to `ping`, would otherwise overwrite an earlier registry entry.
pub fn unique_qualified_tool_name(used: &mut HashSet<String>, server: &str, tool: &str) -> String {
    let base = qualified_tool_name(server, tool);
    let mut candidate = base.clone();
    let mut suffix = 2u32;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

/// `value` with everything but ASCII letters, digits, `_` and `-` replaced by
/// `_`, as a tool name has to be.
pub(super) fn sanitize_identifier(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_unless_already_namespaced() {
        assert_eq!(
            qualified_tool_name("magents", "spawn_session"),
            "magents_spawn_session"
        );
        assert_eq!(
            qualified_tool_name("magents", "magents_spawn_session"),
            "magents_spawn_session"
        );
        assert_eq!(qualified_tool_name("docs", "docs"), "docs");
    }

    #[test]
    fn sanitizes_odd_characters() {
        assert_eq!(
            qualified_tool_name("my.server", "list/files"),
            "my_server_list_files"
        );
    }

    #[test]
    fn disambiguates_colliding_qualified_names() {
        let mut used = HashSet::new();
        assert_eq!(
            unique_qualified_tool_name(&mut used, "srv", "ping"),
            "srv_ping"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "srv", "srv_ping"),
            "srv_ping_2"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "my.server", "list/files"),
            "my_server_list_files"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "my.server", "list_files"),
            "my_server_list_files_2"
        );
    }
}
