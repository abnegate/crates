use std::collections::BTreeMap;

use abnegate_secret::SecretValue;
use tempfile::NamedTempFile;

/// A rendered MCP configuration and what the child needs for it to resolve.
///
/// The file holds no environment or header value but `${VAR}` references,
/// and no stdio server's command or argument that refers to a variable, so a
/// file left behind by a run killed before it could clean up exposes no
/// secret. Each such value moves into a generated variable named
/// `ABNEGATE_MCP_<token>_<n>`, whose token is drawn at random for every
/// rendering: literal text, as it is, into
/// [`environment`](McpAttachment::environment), and a stdio server's value
/// that refers to variables into [`templates`](McpAttachment::templates),
/// which the child is given resolved. No variable is handed to the child
/// under its own name on a server's behalf, so a remote server's reference to
/// one expands only if the caller hands it over.
///
/// A remote server's URL and headers keep their references as written, for
/// the CLI alone to expand under its own rules, and only the literal text
/// around them moves out. Other commands and arguments, and URLs, are written
/// as they are, so a secret belongs in a reference there.
#[derive(Debug)]
#[non_exhaustive]
pub struct McpAttachment {
    /// The rendered file, readable by its owner alone and deleted when this
    /// drops, so this must outlive the child that reads it.
    pub file: NamedTempFile,
    /// Each generated variable that holds literal text, with the text: the
    /// child is given it as it is.
    pub environment: BTreeMap<String, SecretValue>,
    /// Each generated variable that holds a stdio server's value referring
    /// to variables, with the value as configured: the child is given it with
    /// each `${VAR}` and `${VAR:-default}` resolved as the CLI would resolve
    /// it, and a reference nothing resolves left as written.
    pub templates: BTreeMap<String, SecretValue>,
}
