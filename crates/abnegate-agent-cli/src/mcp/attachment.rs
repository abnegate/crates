use std::collections::BTreeMap;
use std::collections::BTreeSet;

use abnegate_secret::SecretValue;
use tempfile::NamedTempFile;

/// A rendered MCP configuration and what the child needs for it to resolve.
///
/// The file holds no environment or header value but `${VAR}` references,
/// so a file left behind by a run killed before it could clean up exposes
/// no secret. A literal value is moved into a generated variable in
/// `environment`; a stdio server's value mixing references with literal
/// text is moved into one in `templates`, which the child is given
/// expanded; a value that is a single whole reference is written as it is.
/// A remote server's header keeps its references as written, for the CLI
/// alone to expand under its own rules, and only the literal text around
/// them moves into `environment`. Every variable a stdio server's values
/// refer to is in `references`, for the child to be given from the host;
/// nothing a remote server's URL or headers refer to ever is. Arguments and
/// URLs are written as they are, so a secret belongs in a reference there.
#[derive(Debug)]
#[non_exhaustive]
pub struct McpAttachment {
    /// Readable by its owner alone and deleted when this drops, so this must
    /// outlive the child that reads it.
    pub file: NamedTempFile,
    pub environment: BTreeMap<String, SecretValue>,
    pub templates: BTreeMap<String, SecretValue>,
    pub references: BTreeSet<String>,
}
