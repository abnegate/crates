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
pub const MAXIMUM_TOOL_NAME_CHARACTERS: usize = 64;

/// Longest the server part of a name may be, short enough that its prefix
/// survives any cut [`MAXIMUM_TOOL_NAME_CHARACTERS`] forces.
const MAXIMUM_SERVER_CHARACTERS: usize = 32;

/// Hexadecimal characters of the digest a cut name ends in.
const DIGEST_CHARACTERS: usize = 8;

const UNNAMED_SERVER: &str = "server";
const UNNAMED_TOOL: &str = "tool";

const UNDERSCORE: char = '_';

/// `server` + `tool` → a function name safe for OpenAI-style tool calling:
/// `server__tool`, at most [`MAXIMUM_TOOL_NAME_CHARACTERS`] long.
///
/// Both parts are named as a coding agent CLI names them in
/// `mcp__<server>__<tool>`: every character but an ASCII letter, a digit,
/// `_` and `-` becomes `_`, and the tool is always prefixed, even one whose
/// own name already starts with its server's, so `notes__search` on `notes`
/// is `notes__notes__search` here as it is `mcp__notes__notes__search` there.
/// A server name the CLI would refuse, one holding `__` or ending in `_`, has
/// its runs of `_` collapsed and its trailing `_` dropped, so the separator
/// still marks where the server's part ends.
///
/// A name that would run long is cut and ends in a digest of the whole, so
/// two long names that share a start still differ, and a server's part is
/// cut to 32 characters first so its prefix survives that cut.
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
    format!(
        "{}{}",
        server_prefix(server),
        sanitize_identifier(tool, UNNAMED_TOOL)
    )
}

/// A server's name as its tools carry it: sanitized, with every run of
/// underscores cut to one and none at its end, so it can never hold the
/// separator, and cut to [`MAXIMUM_SERVER_CHARACTERS`].
fn server_identifier(server: &str) -> String {
    let sanitized = sanitize_identifier(server, UNNAMED_SERVER);
    let mut collapsed = String::with_capacity(sanitized.len());
    for character in sanitized.chars() {
        if character == UNDERSCORE && collapsed.ends_with(UNDERSCORE) {
            continue;
        }
        collapsed.push(character);
    }
    let cut: String = collapsed.chars().take(MAXIMUM_SERVER_CHARACTERS).collect();
    match cut.trim_end_matches(UNDERSCORE) {
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
            if character.is_ascii_alphanumeric() || character == UNDERSCORE || character == '-' {
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
    if name.len() <= MAXIMUM_TOOL_NAME_CHARACTERS {
        return name.to_string();
    }
    let digest = hex::encode(Sha256::digest(name.as_bytes()));
    let kept = MAXIMUM_TOOL_NAME_CHARACTERS - DIGEST_CHARACTERS - 1;
    format!("{}_{}", &name[..kept], &digest[..DIGEST_CHARACTERS])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpServer;

    #[test]
    fn a_tool_is_always_prefixed_even_when_it_already_starts_with_its_server() {
        assert_eq!(
            qualified_tool_name("notes", "search_notes"),
            "notes__search_notes"
        );
        assert_eq!(
            qualified_tool_name("notes", "notes__search_notes"),
            "notes__notes__search_notes"
        );
        assert_eq!(qualified_tool_name("docs", "docs"), "docs__docs");
    }

    /// One `tools` list scopes a CLI and the hub alike only if every server
    /// the CLI attaches names its tools here as the CLI names them there.
    #[test]
    fn a_name_is_the_clis_own_without_its_mcp_prefix() {
        for (server, tool, listed) in [
            ("notes", "search", "search"),
            ("notes", "notes__search", "notes__search"),
            ("_internal", "run", "run"),
            ("my-server", "list.files", "list_files"),
            ("docs", "docs", "docs"),
        ] {
            let scoped = McpServer::command("server", Vec::<String>::new()).with_tools([listed]);
            assert!(scoped.nameable(server), "{server}");
            assert!(scoped.allows(tool), "{tool}");

            assert_eq!(
                [format!("mcp__{}", qualified_tool_name(server, tool))],
                scoped.scoped_tools(server).as_slice(),
                "{server} {tool}"
            );
        }
    }

    /// A tool advertised as `search` and another as `notes__search` are two
    /// tools to a CLI, and must not trade names here by the order a server
    /// lists them in.
    #[test]
    fn a_prefixed_tool_never_takes_the_name_of_the_tool_it_repeats() {
        for order in [["search", "notes__search"], ["notes__search", "search"]] {
            let mut used = HashSet::new();
            let names: Vec<String> = order
                .iter()
                .map(|tool| unique_qualified_tool_name(&mut used, "notes", tool))
                .collect();
            let search = order.iter().position(|tool| *tool == "search").unwrap();

            assert_eq!(names[search], "notes__search", "{order:?}");
            assert_eq!(names[1 - search], "notes__notes__search", "{order:?}");
        }
    }

    #[test]
    fn sanitizes_odd_characters() {
        assert_eq!(
            qualified_tool_name("my.server", "list/files"),
            "my_server__list_files"
        );
    }

    /// A server part that held the separator, or ended on an underscore,
    /// would move where the name's first `__` falls. One leading underscore
    /// cannot, and the CLI keeps it.
    #[test]
    fn a_server_part_never_holds_the_separator() {
        assert_eq!(qualified_tool_name("a__b", "c"), "a_b__c");
        assert_eq!(qualified_tool_name("_run_", "deploy"), "_run__deploy");
        assert_eq!(qualified_tool_name("__run", "deploy"), "_run__deploy");
        assert_eq!(qualified_tool_name("_", "x"), "server__x");
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
            "srv__srv__ping"
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

        assert_eq!(name.len(), MAXIMUM_TOOL_NAME_CHARACTERS);
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
        assert_eq!(second.len(), MAXIMUM_TOOL_NAME_CHARACTERS);
    }
}
