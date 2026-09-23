use std::collections::BTreeSet;
use std::path::Path;

use super::error::ConfinementError;
use super::mode::ConfinementMode;
use super::path::text;
use super::resolved::Resolved;

const READ_TREES: [&str; 4] = ["/System", "/dev", "/usr/lib", "/usr/share"];

/// `/bin/sh` resolves which shell binary to become by reading this directory,
/// so a tree driven through a shell cannot start without it. Single-command
/// mode does not need it because nothing re-execs.
const TREE_READ_TREES: [&str; 1] = ["/private/var/select"];

/// Name resolution and account enumeration stay denied even though `system.sb`
/// grants a broad read of the system volume.
pub(super) const DENIED_FILES: [&str; 2] = ["/private/etc/hosts", "/private/etc/passwd"];

pub(super) fn arguments(resolved: &Resolved) -> Result<Vec<String>, ConfinementError> {
    let command = text(&resolved.command)?;
    let mut arguments = vec![
        "-p".to_string(),
        profile(resolved)?,
        "--".to_string(),
        command.to_string(),
    ];
    arguments.extend(resolved.arguments.iter().cloned());
    Ok(arguments)
}

fn profile(resolved: &Resolved) -> Result<String, ConfinementError> {
    let command = text(&resolved.command)?;
    let tree = resolved.mode == ConfinementMode::ProcessTree;

    let mut trees: BTreeSet<&str> = READ_TREES.into_iter().collect();
    for root in resolved.read_roots.iter().chain(&resolved.write_roots) {
        trees.insert(text(root)?);
    }
    if tree {
        trees.extend(TREE_READ_TREES);
        for root in &resolved.execute_roots {
            trees.insert(text(root)?);
        }
    }

    let mut metadata: BTreeSet<&Path> = BTreeSet::new();
    for path in trees
        .iter()
        .map(Path::new)
        .chain([resolved.command.as_path()])
    {
        metadata.extend(
            path.ancestors()
                .filter(|ancestor| ancestor.parent().is_some()),
        );
    }

    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        "(import \"system.sb\")".to_string(),
        format!("(allow process-exec (literal {}))", escape(command)),
        "(deny signal)".to_string(),
        "(allow sysctl-read)".to_string(),
    ];
    if tree {
        // A later clause overrides an earlier one, so this narrows the blanket
        // `(deny signal)` above to the tree's own descendants: `cargo` may stop
        // a test binary it started, and may not touch anything else on the host.
        lines.push("(allow process-fork)".to_string());
        lines.push("(allow signal (target children))".to_string());
        for root in &resolved.execute_roots {
            lines.push(format!(
                "(allow process-exec (subpath {}))",
                escape(text(root)?)
            ));
        }
    }
    for tree in &trees {
        lines.push(format!("(allow file-read* (subpath {}))", escape(tree)));
    }
    lines.push(format!("(allow file-read* (literal {}))", escape(command)));
    for path in &metadata {
        lines.push(format!(
            "(allow file-read-metadata (literal {}))",
            escape(text(path)?)
        ));
    }
    for root in &resolved.write_roots {
        lines.push(format!(
            "(allow file-write* (subpath {}))",
            escape(text(root)?)
        ));
    }
    lines.push("(deny network*)".to_string());
    for path in DENIED_FILES {
        lines.push(format!("(deny file-read* (literal {}))", escape(path)));
    }

    Ok(lines.join("\n"))
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        if character == '"' || character == '\\' {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_plain_path() {
        assert_eq!(escape("/tmp/work"), "\"/tmp/work\"");
    }

    #[test]
    fn test_escape_quotes_and_backslashes() {
        assert_eq!(escape("/tmp/a\"b\\c d"), "\"/tmp/a\\\"b\\\\c d\"");
    }

    #[test]
    fn test_escape_closing_backslash_cannot_escape_the_terminator() {
        let escaped = escape("/tmp/trailing\\");
        assert_eq!(escaped, "\"/tmp/trailing\\\\\"");
        assert_eq!(escaped.matches('"').count(), 2);
    }

    #[test]
    fn test_escape_preserves_unicode() {
        assert_eq!(escape("/tmp/日本語"), "\"/tmp/日本語\"");
    }
}
