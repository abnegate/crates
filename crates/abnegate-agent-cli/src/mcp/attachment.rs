use std::collections::BTreeMap;
use std::collections::BTreeSet;

use abnegate_secret::SecretValue;
use tempfile::NamedTempFile;

/// A rendered MCP configuration and what the child needs for it to resolve.
///
/// The file holds no literal secret: each literal environment or header
/// value is written as a `${VAR}` reference to a generated variable in
/// `environment`, which the child is given, so a file left behind by a run
/// killed before it could clean up exposes nothing. A value that already
/// holds a reference is written as it is, and the variables it names are in
/// `references` for the child to be given from the host.
#[derive(Debug)]
#[non_exhaustive]
pub struct McpAttachment {
    /// Readable by its owner alone and deleted when this drops, so this must
    /// outlive the child that reads it.
    pub file: NamedTempFile,
    pub environment: BTreeMap<String, SecretValue>,
    pub references: BTreeSet<String>,
}
