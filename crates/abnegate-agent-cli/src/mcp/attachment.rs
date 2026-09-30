use std::collections::BTreeMap;

use abnegate_secret::SecretValue;
use tempfile::NamedTempFile;

use crate::mcp::template::Template;

/// A rendered MCP configuration and what the child needs for it to resolve.
///
/// The configuration is a file for an agent that reads one, and `-c`
/// overrides on its command line for Codex. Neither holds an environment or
/// header value, a URL that refers to a variable, or a stdio command or
/// argument that does, but a reference to a generated variable named
/// `ABNEGATE_MCP_<token>_<n>`, whose token is drawn at random for every
/// rendering, so a file left behind by a run killed before it could clean up,
/// or a command line another user lists, exposes no secret. Literal text, and
/// a remote server's URL and header values resolved against its own secrets,
/// go into [`environment`](McpAttachment::environment) as they are, and a
/// stdio server's values that refer to variables into
/// [`templates`](McpAttachment::templates), which the child is given
/// resolved. Other commands and arguments, and URLs, are written as they
/// are, so a secret belongs in a reference there.
#[derive(Debug)]
pub(crate) struct McpAttachment {
    /// The rendered file, for an agent that reads one, readable by its owner
    /// alone and deleted when this drops, so this must outlive the child
    /// that reads it.
    pub(crate) file: Option<NamedTempFile>,
    /// The `-c` overrides that attach each server to Codex.
    pub(crate) overrides: Vec<String>,
    /// Each generated variable the child is given as it is, with its value.
    pub(crate) environment: BTreeMap<String, SecretValue>,
    /// Each generated variable that holds a stdio server's value referring
    /// to variables, with the value as configured and the server's secrets:
    /// the child is given it with each `${VAR}` and `${VAR:-default}`
    /// resolved as the CLI would resolve it, a secret bound for the variable
    /// read first, and a reference nothing resolves left as written.
    pub(crate) templates: BTreeMap<String, Template>,
    /// Every secret bound to a server the configuration holds, which the run
    /// scrubs from what it writes down on its own as well as within a value.
    pub(crate) secrets: Vec<SecretValue>,
}
