use crate::mcp::inline_table::InlineTable;

/// How Codex starts one stdio server: the program, what follows it, and the
/// generated variables it hands the server under their own names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) command: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) variables: Vec<String>,
}

impl Launch {
    /// The launch as Codex's `mcp_servers.<name>` table holds it.
    pub(crate) fn table(&self) -> InlineTable {
        let table = InlineTable::default()
            .with_text("command", &self.command)
            .with_texts("args", &self.arguments);
        match self.variables.is_empty() {
            true => table,
            false => table.with_texts("env_vars", &self.variables),
        }
    }
}
