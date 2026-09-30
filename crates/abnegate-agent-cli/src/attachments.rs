//! What a run hands its agent beside its settings.

use std::path::Path;

/// What [`AgentKind::options`](crate::AgentKind::options) hands the agent
/// beside its settings: the rendered MCP configuration, as a file for Claude
/// and as `-c` overrides for Codex, and the instructions appended to its
/// system prompt, which go in a file because `argv` has a hard size limit
/// that instructions can reach.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Attachments<'a> {
    pub mcp: Option<&'a Path>,
    pub instructions: Option<&'a Path>,
    /// The `-c mcp_servers.<name>={...}` overrides that attach MCP servers
    /// to Codex, each flag and its value in turn. They hold no value that may
    /// be secret, only the names of the generated variables that do.
    pub mcp_overrides: &'a [String],
}

impl<'a> Attachments<'a> {
    pub fn with_mcp(mut self, path: &'a Path) -> Self {
        self.mcp = Some(path);
        self
    }

    pub fn with_instructions(mut self, path: &'a Path) -> Self {
        self.instructions = Some(path);
        self
    }

    /// The same attachments, handing Codex `overrides`: see
    /// [`Attachments::mcp_overrides`].
    pub fn with_mcp_overrides(mut self, overrides: &'a [String]) -> Self {
        self.mcp_overrides = overrides;
        self
    }
}
