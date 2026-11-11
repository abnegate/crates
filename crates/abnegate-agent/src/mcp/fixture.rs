pub(super) const MCP_PATH: &str = "/mcp";

/// Nothing listens on port 1.
pub(super) const UNREACHABLE: &str = "http://127.0.0.1:1";

/// A name no name server knows, so only a resolver a test installs or a
/// proxy can reach it.
pub(super) const UNRESOLVABLE: &str = "http://mcp.invalid/mcp";
