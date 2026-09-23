use std::collections::HashSet;

use sha2::Digest;
use sha2::Sha256;

/// What stands between a server's name and its tool's in a qualified name.
///
/// Two underscores rather than one, and never inside a server's own part, so
/// the first occurrence in a name says exactly which server it came from: a
/// server named `run` claims `run__deploy` and never the built-in
/// `run_command`.
pub const SEPARATOR: &str = "__";

/// Longest name an OpenAI-style function may have.
pub const MAX_TOOL_NAME_CHARACTERS: usize = 64;

/// Longest the server part of a name may be, short enough that its prefix
/// survives any cut [`MAX_TOOL_NAME_CHARACTERS`] forces.
const MAX_SERVER_CHARACTERS: usize = 32;

/// Hex characters of the digest a cut name ends in.
const DIGEST_CHARACTERS: usize = 8;

const UNNAMED_SERVER: &str = "server";
const UNNAMED_TOOL: &str = "tool";

/// `server` + `tool` → a function name safe for OpenAI-style tool calling:
/// `server__tool`, at most [`MAX_TOOL_NAME_CHARACTERS`] long.
///
/// A tool already named under its server's prefix keeps its name. A name
/// that would run long is cut and ends in a digest of the whole, so two long
/// names that share a start still differ.
pub fn qualified_tool_name(server: &str, tool: &str) -> String {
    fit(&joined(server, tool))
}

/// Same as [`qualified_tool_name`], then `_2`, `_3`, … if that name is taken.
///
/// Sanitizing `list.files` and `list_files` would otherwise overwrite an
/// earlier registry entry.
pub fn unique_qualified_tool_name(used: &mut HashSet<String>, server: &str, tool: &str) -> String {
    let base = joined(server, tool);
    let mut candidate = fit(&base);
    let mut suffix = 2u32;
    while !used.insert(candidate.clone()) {
        candidate = fit(&format!("{base}_{suffix}"));
        suffix += 1;
    }
    candidate
}

/// The prefix every tool attached from `server` is named under.
pub(super) fn server_prefix(server: &str) -> String {
    format!("{}{SEPARATOR}", server_identifier(server))
}

fn joined(server: &str, tool: &str) -> String {
    let prefix = server_prefix(server);
    let tool = sanitize_identifier(tool, UNNAMED_TOOL);
    if tool.starts_with(&prefix) {
        tool
    } else {
        format!("{prefix}{tool}")
    }
}

/// A server's name as its tools carry it: sanitized, with no run of
/// underscores inside it and none at either end, so it can never hold the
/// separator, and cut to [`MAX_SERVER_CHARACTERS`].
fn server_identifier(server: &str) -> String {
    let sanitized = sanitize_identifier(server, UNNAMED_SERVER);
    let collapsed: Vec<&str> = sanitized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect();
    let joined = collapsed.join("_");
    let cut: String = joined.chars().take(MAX_SERVER_CHARACTERS).collect();
    match cut.trim_end_matches('_') {
        "" => UNNAMED_SERVER.to_string(),
        identifier => identifier.to_string(),
    }
}

/// `value` with everything but ASCII letters, digits, `_` and `-` replaced by
/// `_`, as a tool name has to be, or `fallback` when nothing is left.
fn sanitize_identifier(value: &str, fallback: &str) -> String {
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
        fallback.to_string()
    } else {
        sanitized
    }
}

/// `name`, or its start and a digest of the whole when it is too long.
fn fit(name: &str) -> String {
    if name.len() <= MAX_TOOL_NAME_CHARACTERS {
        return name.to_string();
    }
    let digest = hex::encode(Sha256::digest(name.as_bytes()));
    let kept = MAX_TOOL_NAME_CHARACTERS - DIGEST_CHARACTERS - 1;
    format!("{}_{}", &name[..kept], &digest[..DIGEST_CHARACTERS])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_unless_already_namespaced() {
        assert_eq!(
            qualified_tool_name("notes", "search_notes"),
            "notes__search_notes"
        );
        assert_eq!(
            qualified_tool_name("notes", "notes__search_notes"),
            "notes__search_notes"
        );
        assert_eq!(qualified_tool_name("docs", "docs"), "docs__docs");
    }

    #[test]
    fn sanitizes_odd_characters() {
        assert_eq!(
            qualified_tool_name("my.server", "list/files"),
            "my_server__list_files"
        );
    }

    /// A server part that held the separator, or began or ended on an
    /// underscore, would move where the name's first `__` falls.
    #[test]
    fn a_server_part_never_holds_the_separator() {
        assert_eq!(qualified_tool_name("a__b", "c"), "a_b__c");
        assert_eq!(qualified_tool_name("_run_", "deploy"), "run__deploy");
        assert_eq!(qualified_tool_name("...", "x"), "server__x");
        assert_eq!(qualified_tool_name("", ""), "server__tool");
    }

    #[test]
    fn disambiguates_colliding_qualified_names() {
        let mut used = HashSet::new();
        assert_eq!(
            unique_qualified_tool_name(&mut used, "srv", "ping"),
            "srv__ping"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "srv", "srv__ping"),
            "srv__ping_2"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "my.server", "list/files"),
            "my_server__list_files"
        );
        assert_eq!(
            unique_qualified_tool_name(&mut used, "my.server", "list_files"),
            "my_server__list_files_2"
        );
    }

    /// A provider refuses a function name past 64 characters, and refuses
    /// the whole request with it.
    #[test]
    fn a_long_name_is_cut_the_same_way_every_time_and_keeps_its_prefix() {
        let server = "a-server-with-a-name-that-goes-on-and-on-and-on";
        let long = "search_every_document_this_server_has_ever_indexed_for_the_phrase";
        let other = "search_every_document_this_server_has_ever_indexed_for_the_title";

        let name = qualified_tool_name(server, long);

        assert_eq!(name.len(), MAX_TOOL_NAME_CHARACTERS);
        assert_eq!(name, qualified_tool_name(server, long), "the cut is stable");
        assert_ne!(
            name,
            qualified_tool_name(server, other),
            "the digest tells them apart"
        );
        assert!(name.starts_with(&server_prefix(server)), "{name}");
        assert!(
            name.chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
        );

        let mut used = HashSet::from([name.clone()]);
        let second = unique_qualified_tool_name(&mut used, server, long);
        assert_ne!(second, name);
        assert_eq!(second.len(), MAX_TOOL_NAME_CHARACTERS);
    }
}
