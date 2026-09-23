//! Files a run hands its agent by path rather than on its command line.

use std::path::Path;

/// The files [`AgentKind::options`](crate::AgentKind::options) points the
/// agent at: the rendered MCP configuration, and the instructions appended
/// to its system prompt, which go in a file because `argv` has a hard size
/// limit that instructions can reach.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Attachments<'a> {
    pub mcp: Option<&'a Path>,
    pub instructions: Option<&'a Path>,
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
}
